#!/usr/bin/env bash
# Extrait une piste de sous-titres française d'un MKV à codec identique, sur la seedbox (disque local, rien sur le
# lien VPS). La piste est choisie ici par ffprobe (les index Jellyfin bougent dès qu'un fichier externe existe).
# Usage : gc-extract-sub.sh <vidéo> <codec ass|subrip> <full|forced|hi> <fichier de sortie> [srt-sans-panneaux]
#   full = ni forcée ni malentendants (la piste « default » d'abord) ; forced ; hi = malentendants.
#   Sortie écrite via un fichier temporaire puis renommée. Si un 5e argument (.srt) est donné et que la sortie est un
#   .ass, gc-ass2srt.py en dérive un SRT sans les lignes de panneaux (AirPlay, téléviseurs).
# Idempotent : sortie déjà présente et non vide → rien (code 0). Codes : 2 vidéo absente, 3 piste introuvable/vide.
set -euo pipefail
video=$1; codec=$2; kind=$3; out=$4; srt=${5:-}
[ -f "$video" ] || { echo "vidéo absente : $video" >&2; exit 2; }
fresh=0
if [ ! -s "$out" ]; then
  fresh=1
  idx=$(ffprobe -v error -print_format json -show_streams -select_streams s "$video" | python3 -c '
import sys, json
codec, kind = sys.argv[1], sys.argv[2]
best = None
for s in json.load(sys.stdin).get("streams", []):
    lang = (s.get("tags") or {}).get("language", "").lower()
    if lang not in ("fre", "fra", "fr") or s.get("codec_name") != codec:
        continue
    d = s.get("disposition") or {}
    forced, hi = bool(d.get("forced")), bool(d.get("hearing_impaired"))
    title = ((s.get("tags") or {}).get("title") or "").lower()
    hi = hi or "malentendant" in title or "sdh" in title or "[cc]" in title
    forced = forced or "forc" in title
    if kind == "forced" and not forced: continue
    if kind == "hi" and not hi: continue
    if kind == "full" and (forced or hi): continue
    score = (1 if d.get("default") else 0)
    if best is None or score > best[0]:
        best = (score, s["index"])
print(best[1] if best else "")' "$codec" "$kind")
  [ -n "$idx" ] || { echo "aucune piste $codec/$kind : $video" >&2; exit 3; }
  tmp="${out%.*}.gc-tmp.${out##*.}"
  nice -n 19 ionice -c 3 ffmpeg -nostdin -hide_banner -loglevel error -y -i "$video" -map "0:$idx" -an -vn -c:s copy "$tmp"
  [ -s "$tmp" ] || { rm -f "$tmp"; echo "extraction vide : $video flux $idx" >&2; exit 3; }
  mv -f "$tmp" "$out"
fi
# SRT dérivé : (re)généré quand l'ASS vient d'être extrait (remplace un SRT converti ailleurs, avec panneaux)
if [ -n "$srt" ] && { [ "$fresh" = 1 ] || [ ! -s "$srt" ]; }; then
  case "$out" in
    *.ass|*.ssa) python3 "$(dirname "$0")/gc-ass2srt.py" "$out" "$srt.gc-tmp" && mv -f "$srt.gc-tmp" "$srt" ;;
  esac
fi
echo "ok $(stat -c %s "$out") $out"
