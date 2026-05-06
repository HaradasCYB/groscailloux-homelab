#!/bin/bash
# Auto-import des fichiers déposés dans /opt/homelab/downloads par pyload (ou autre).
# Le script tourne sur l'HÔTE (pas dans un conteneur) → on utilise localhost:port et
# on convertit le filepath de /opt/homelab/downloads/* vers /downloads/* (vu par les Arr).
#
# Pour chaque fichier vidéo détecté :
#   1. Parse le nom via /api/v3/parse (Radarr ou Sonarr selon TV/Movie)
#   2. Lookup TMDB/TVDB via /lookup → tmdbId/tvdbId
#   3. Si pas dans la library : POST /api/v3/movie ou /series avec searchForXxx:false
#   4. Trigger DownloadedXxxScan pour l'import (hardlink vers /movies ou /tv)

# shellcheck disable=SC1091
[ -f /opt/homelab/.env ] && set -a && . /opt/homelab/.env && set +a

WATCH_DIR="${HOMELAB_DOWNLOADS_DIR:-/opt/homelab/downloads}"
RADARR_API="${RADARR_API_KEY}"
SONARR_API="${SONARR_API_KEY}"
RADARR_URL="${RADARR_URL:-http://localhost:7878}"
SONARR_URL="${SONARR_URL:-http://localhost:8989}"
LOG="${HOMELAB_LOGS_DIR:-/opt/homelab/logs}/auto-import.log"

QUALITY_PROFILE_ID="${QUALITY_PROFILE_ID:-6}"
RADARR_ROOT="${RADARR_ROOT:-/movies}"
SONARR_ROOT="${SONARR_ROOT:-/tv}"

mkdir -p /opt/homelab/logs

ts() { date '+%Y-%m-%d %H:%M:%S'; }
log_msg() { echo "$(ts): $*" >> "$LOG"; }

urlencode() {
  jq -rn --arg s "$1" '$s | @uri'
}

ensure_movie_in_radarr() {
  local filename="$1"

  local parse_resp
  parse_resp=$(curl -fsS --max-time 30 -H "X-Api-Key: $RADARR_API" \
    "$RADARR_URL/api/v3/parse?title=$(urlencode "$filename")" 2>/dev/null) \
    || { log_msg "    parse: HTTP error"; return 0; }

  local movie_title year
  movie_title=$(echo "$parse_resp" | jq -r '.parsedMovieInfo.movieTitles[0] // empty')
  year=$(echo "$parse_resp" | jq -r '.parsedMovieInfo.year // empty')

  if [ -z "$movie_title" ]; then
    log_msg "    parse: no movie title extracted"
    return 0
  fi

  local lookup_term="$movie_title"
  [ -n "$year" ] && [ "$year" != "0" ] && lookup_term="$movie_title $year"

  local lookup_resp
  lookup_resp=$(curl -fsS --max-time 30 -H "X-Api-Key: $RADARR_API" \
    "$RADARR_URL/api/v3/movie/lookup?term=$(urlencode "$lookup_term")" 2>/dev/null) \
    || { log_msg "    lookup: HTTP error term='$lookup_term'"; return 0; }

  local tmdb_id found_title found_year
  tmdb_id=$(echo "$lookup_resp" | jq -r '.[0].tmdbId // empty')
  found_title=$(echo "$lookup_resp" | jq -r '.[0].title // empty')
  found_year=$(echo "$lookup_resp" | jq -r '.[0].year // empty')

  if [ -z "$tmdb_id" ] || [ "$tmdb_id" = "0" ]; then
    log_msg "    lookup: no_match for '$lookup_term'"
    return 0
  fi

  local existing
  existing=$(curl -fsS --max-time 15 -H "X-Api-Key: $RADARR_API" \
    "$RADARR_URL/api/v3/movie?tmdbId=$tmdb_id" 2>/dev/null) || existing="[]"

  if [ "$(echo "$existing" | jq 'length')" != "0" ]; then
    log_msg "    library: already_in_library tmdbId=$tmdb_id title='$found_title' ($found_year)"
    return 0
  fi

  local body
  body=$(jq -n --argjson tmdb "$tmdb_id" --arg title "$found_title" --argjson yr "$found_year" \
    --argjson qp "$QUALITY_PROFILE_ID" --arg root "$RADARR_ROOT" '{
      tmdbId: $tmdb,
      title: $title,
      year: $yr,
      qualityProfileId: $qp,
      rootFolderPath: $root,
      monitored: true,
      minimumAvailability: "released",
      addOptions: {searchForMovie: false, monitor: "movieOnly"}
    }')

  local add_resp add_id
  add_resp=$(curl -fsS --max-time 30 -X POST \
    -H "X-Api-Key: $RADARR_API" -H "Content-Type: application/json" \
    -d "$body" "$RADARR_URL/api/v3/movie" 2>/dev/null) \
    || { log_msg "    auto_add: HTTP error tmdbId=$tmdb_id"; return 0; }

  add_id=$(echo "$add_resp" | jq -r '.id // empty')
  if [ -n "$add_id" ]; then
    log_msg "    auto_added tmdbId=$tmdb_id title='$found_title' ($found_year) radarrId=$add_id"
  else
    log_msg "    auto_add: failed body=$(echo "$add_resp" | head -c 200)"
  fi
}

ensure_series_in_sonarr() {
  local filename="$1"

  local parse_resp
  parse_resp=$(curl -fsS --max-time 30 -H "X-Api-Key: $SONARR_API" \
    "$SONARR_URL/api/v3/parse?title=$(urlencode "$filename")" 2>/dev/null) \
    || { log_msg "    parse: HTTP error"; return 0; }

  local series_title
  series_title=$(echo "$parse_resp" | jq -r '.parsedEpisodeInfo.seriesTitle // empty')

  if [ -z "$series_title" ]; then
    log_msg "    parse: no series title extracted"
    return 0
  fi

  local lookup_resp
  lookup_resp=$(curl -fsS --max-time 30 -H "X-Api-Key: $SONARR_API" \
    "$SONARR_URL/api/v3/series/lookup?term=$(urlencode "$series_title")" 2>/dev/null) \
    || { log_msg "    lookup: HTTP error term='$series_title'"; return 0; }

  local tvdb_id found_title found_year
  tvdb_id=$(echo "$lookup_resp" | jq -r '.[0].tvdbId // empty')
  found_title=$(echo "$lookup_resp" | jq -r '.[0].title // empty')
  found_year=$(echo "$lookup_resp" | jq -r '.[0].year // empty')

  if [ -z "$tvdb_id" ] || [ "$tvdb_id" = "0" ]; then
    log_msg "    lookup: no_match for '$series_title'"
    return 0
  fi

  local existing
  existing=$(curl -fsS --max-time 15 -H "X-Api-Key: $SONARR_API" \
    "$SONARR_URL/api/v3/series?tvdbId=$tvdb_id" 2>/dev/null) || existing="[]"

  if [ "$(echo "$existing" | jq 'length')" != "0" ]; then
    log_msg "    library: already_in_library tvdbId=$tvdb_id title='$found_title' ($found_year)"
    return 0
  fi

  local body
  body=$(jq -n --argjson tvdb "$tvdb_id" --arg title "$found_title" --argjson yr "$found_year" \
    --argjson qp "$QUALITY_PROFILE_ID" --arg root "$SONARR_ROOT" '{
      tvdbId: $tvdb,
      title: $title,
      year: $yr,
      qualityProfileId: $qp,
      rootFolderPath: $root,
      monitored: true,
      seasonFolder: true,
      addOptions: {
        searchForMissingEpisodes: false,
        searchForCutoffUnmetEpisodes: false,
        monitor: "all"
      }
    }')

  local add_resp add_id
  add_resp=$(curl -fsS --max-time 30 -X POST \
    -H "X-Api-Key: $SONARR_API" -H "Content-Type: application/json" \
    -d "$body" "$SONARR_URL/api/v3/series" 2>/dev/null) \
    || { log_msg "    auto_add: HTTP error tvdbId=$tvdb_id"; return 0; }

  add_id=$(echo "$add_resp" | jq -r '.id // empty')
  if [ -n "$add_id" ]; then
    log_msg "    auto_added tvdbId=$tvdb_id title='$found_title' ($found_year) sonarrId=$add_id"
  else
    log_msg "    auto_add: failed body=$(echo "$add_resp" | head -c 200)"
  fi
}

trigger_radarr_scan() {
  local filepath_container="$1"
  local code
  code=$(curl -sS -o /tmp/auto-import-resp.json -w '%{http_code}' \
    -X POST "$RADARR_URL/api/v3/command" \
    -H "X-Api-Key: $RADARR_API" \
    -H "Content-Type: application/json" \
    -d "{\"name\":\"DownloadedMoviesScan\",\"path\":\"$filepath_container\",\"importMode\":\"auto\"}")
  log_msg "    scan: HTTP=$code body=$(head -c 200 /tmp/auto-import-resp.json)"
}

trigger_sonarr_scan() {
  local filepath_container="$1"
  local code
  code=$(curl -sS -o /tmp/auto-import-resp.json -w '%{http_code}' \
    -X POST "$SONARR_URL/api/v3/command" \
    -H "X-Api-Key: $SONARR_API" \
    -H "Content-Type: application/json" \
    -d "{\"name\":\"DownloadedEpisodesScan\",\"path\":\"$filepath_container\",\"importMode\":\"auto\"}")
  log_msg "    scan: HTTP=$code body=$(head -c 200 /tmp/auto-import-resp.json)"
}

classify_and_scan() {
    local label="$1"        # base used for parse/lookup (basename of file, or first video inside an archive)
    local scan_path="$2"    # path passed to Sonarr/Radarr scan (in container view)

    if echo "$label" | grep -qiE 'S[0-9]{1,2}E[0-9]{1,2}|[0-9]{1,2}x[0-9]{1,2}|Season|Saison|Complete'; then
        log_msg " → TV Series, ensuring in Sonarr library + scan"
        ensure_series_in_sonarr "$label"
        sleep 3
        trigger_sonarr_scan "$scan_path"
    else
        log_msg " → Movie, ensuring in Radarr library + scan"
        ensure_movie_in_radarr "$label"
        sleep 3
        trigger_radarr_scan "$scan_path"
    fi
}

handle_archive() {
    local filename="$1"
    local filepath_host="$WATCH_DIR/$filename"

    log_msg "Archive detected: $filename, waiting for size stability"
    local prev=-1 stable=0 i=0
    while [ $i -lt 360 ]; do
        [ -f "$filepath_host" ] || { log_msg "    vanished during wait: $filename"; return 1; }
        local curr; curr=$(stat -c %s "$filepath_host" 2>/dev/null || echo 0)
        if [ "$curr" -gt 0 ] && [ "$curr" = "$prev" ]; then
            stable=$((stable + 1))
            [ $stable -ge 3 ] && break
        else
            stable=0
        fi
        prev=$curr
        sleep 5
        i=$((i + 1))
    done
    log_msg "    archive size stable ($prev bytes), extracting"

    local stem="${filename%.*}"
    local destdir="$WATCH_DIR/$stem"
    mkdir -p "$destdir" || { log_msg "    mkdir failed: $destdir"; return 1; }

    case "$filename" in
        *.zip|*.ZIP)
            unzip -q -o "$filepath_host" -d "$destdir" || { log_msg "    unzip failed"; return 1; }
            ;;
        *.rar|*.RAR)
            if command -v unrar >/dev/null 2>&1; then
                unrar x -o+ -y "$filepath_host" "$destdir/" >/dev/null || { log_msg "    unrar failed"; return 1; }
            else
                7z x -y -o"$destdir" "$filepath_host" >/dev/null || { log_msg "    7z extract failed"; return 1; }
            fi
            ;;
    esac
    log_msg "    extracted to: $destdir"
    rm -f "$filepath_host" && log_msg "    archive removed"

    local first_video
    first_video=$(find "$destdir" -maxdepth 3 -type f \( -iname '*.mkv' -o -iname '*.mp4' -o -iname '*.avi' \) | head -1)
    if [ -z "$first_video" ]; then
        log_msg "    no video file in extracted dir, no scan triggered"
        return 0
    fi

    local label
    label=$(basename "$first_video")
    classify_and_scan "$label" "/downloads/$stem"
}

log_msg "Starting auto-import watcher..."

inotifywait -m -e create,close_write,moved_to --format "%f" "$WATCH_DIR" |
while read -r filename; do
    filepath_host="$WATCH_DIR/$filename"
    filepath_container="/downloads/$filename"

    case "$filename" in
        *.mkv|*.mp4|*.avi|*.MKV|*.MP4|*.AVI)
            sleep 5
            [ -f "$filepath_host" ] || { log_msg "vanished: $filename"; continue; }
            log_msg "Detected video: $filename"
            classify_and_scan "$filename" "$filepath_container"
            ;;
        *.zip|*.ZIP|*.rar|*.RAR)
            handle_archive "$filename" || true
            ;;
        *) continue ;;
    esac
done
