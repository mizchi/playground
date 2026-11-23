#!/usr/bin/python3.12
"""AT-SPI のクライアントとして、ホストのアクセシビリティの口を問い合わせる (e2e/native-a11y.ts が使う)。結果は JSON

  atspi-probe.py text-at <道> <offset> <boundary>   GetTextAtOffset (boundary: 0=char 1=word 5=line)
  atspi-probe.py string-at <道> <offset> <granularity>
  atspi-probe.py at-point <道> <x> <y>              GetAccessibleAtPoint の名前
  atspi-probe.py insert <道> <pos> <文字> <バイト数>
  atspi-probe.py size <道>                           GetPosition / GetSize
"""
import json
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402


def find_app(name):
    d = Atspi.get_desktop(0)
    for i in range(d.get_child_count()):
        a = d.get_child_at_index(i)
        if a and a.get_name() == name:
            return a
    raise SystemExit(json.dumps({"error": "no app"}))


def node_at(app, path):
    n = app
    for p in [int(x) for x in path.split(".") if x]:
        n = n.get_child_at_index(p)
    return n


cmd, path = sys.argv[1], sys.argv[2]
n = node_at(find_app("AI-native desktop"), path)
if cmd == "text-at":
    r = Atspi.Text.get_text_at_offset(n, int(sys.argv[3]), Atspi.TextBoundaryType(int(sys.argv[4])))
    print(json.dumps({"text": r.content, "start": r.start_offset, "end": r.end_offset}, ensure_ascii=False))
elif cmd == "string-at":
    r = Atspi.Text.get_string_at_offset(n, int(sys.argv[3]), Atspi.TextGranularity(int(sys.argv[4])))
    print(json.dumps({"text": r.content, "start": r.start_offset, "end": r.end_offset}, ensure_ascii=False))
elif cmd == "at-point":
    c = Atspi.Component.get_accessible_at_point(n, int(sys.argv[3]), int(sys.argv[4]), Atspi.CoordType.WINDOW)
    print(json.dumps({"name": c.get_name() if c else None}, ensure_ascii=False))
elif cmd == "insert":
    ok = Atspi.EditableText.insert_text(n, int(sys.argv[3]), sys.argv[4], int(sys.argv[5]))
    print(json.dumps({"ok": ok}))
elif cmd == "size":
    p = Atspi.Component.get_position(n, Atspi.CoordType.WINDOW)
    s = Atspi.Component.get_size(n)
    print(json.dumps({"x": p.x, "y": p.y, "w": s.x, "h": s.y}))
