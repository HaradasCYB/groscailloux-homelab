#!/usr/bin/env python3
"""ASS → SRT « dialogues seulement », pour AirPlay et les téléviseurs (qui ne rendent pas l'ASS).

Les lignes dont le style est un panneau (Sign, Title, OP, ED, Credit, Song, Karaoke, Logo…) sont ignorées : converties
en SRT elles perdent leur position et s'affichent comme des sous-titres ordinaires (« ÉPISODE 4 », titres en plein
écran), ce qui ressemble à des mentions pour malentendants. Les balises {\\…} sont retirées, \\N devient un retour à la
ligne, les lignes vides sont sautées, les lignes identiques et simultanées fusionnées.
Usage : gc-ass2srt.py <entrée.ass> <sortie.srt>
"""
import re
import sys

SIGN_STYLES = re.compile(r"^(sign|signs|title|titre|op|ed|opening|ending|credit|song|karaoke|kara|logo|caption|typeset|ts)", re.I)


def ts(s: str) -> str:
    h, m, rest = s.split(":")
    sec, cs = rest.split(".")
    return "%02d:%02d:%02d,%03d" % (int(h), int(m), int(sec), int(cs.ljust(3, "0")[:3]))


def main(src: str, dst: str) -> int:
    fmt = None
    cues = []
    with open(src, encoding="utf-8-sig", errors="replace") as f:
        for line in f:
            line = line.rstrip("\r\n")
            if line.startswith("Format:") and fmt is None and "Start" in line:
                fmt = [x.strip().lower() for x in line[7:].split(",")]
            elif line.startswith("Dialogue:") and fmt:
                parts = line[9:].split(",", len(fmt) - 1)
                if len(parts) < len(fmt):
                    continue
                row = dict(zip(fmt, [p.strip() for p in parts[:-1]] + [parts[-1]]))
                if SIGN_STYLES.match(row.get("style", "")):
                    continue
                text = re.sub(r"\{[^}]*\}", "", row["text"])
                text = text.replace("\\N", "\n").replace("\\n", "\n").replace("\\h", " ").strip()
                if not text:
                    continue
                cues.append((row["start"], row["end"], text))
    cues.sort(key=lambda c: c[0])
    merged = []
    for c in cues:
        if merged and merged[-1][0] == c[0] and merged[-1][1] == c[1]:
            if c[2] not in merged[-1][2]:
                merged[-1] = (c[0], c[1], merged[-1][2] + "\n" + c[2])
            continue
        merged.append(c)
    with open(dst, "w", encoding="utf-8") as out:
        for i, (a, b, t) in enumerate(merged, 1):
            out.write("%d\n%s --> %s\n%s\n\n" % (i, ts(a), ts(b), t))
    return 0 if merged else 4


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
