#!/usr/bin/python3.12
"""AT-SPI の橋渡し。既存アプリのアクセシビリティツリーを JSON で書き出し、操作を実行する。

  atspi_bridge.py dump <アプリ名>
  atspi_bridge.py action <アプリ名> <道 (0.1.2)> <action 名>
  atspi_bridge.py set-text <アプリ名> <道> <文字列>

道は、アプリのルートからの子の番号の列。結果は JSON で標準出力に出す。
"""
import json
import sys

import pyatspi


def find_app(name):
    desktop = pyatspi.Registry.getDesktop(0)
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        if app is not None and app.name == name:
            return app
    raise SystemExit(json.dumps({"error": f"アプリ {name} がない", "apps": [desktop.getChildAtIndex(i).name for i in range(desktop.childCount) if desktop.getChildAtIndex(i)]}))


def node_at(app, path):
    n = app
    for p in [int(x) for x in path.split(".") if x != ""]:
        n = n.getChildAtIndex(p)
    return n


def dump(acc, path="", depth=0):
    states = acc.getState().getStates()
    st = [pyatspi.stateToString(s) for s in states]
    actions = []
    try:
        ai = acc.queryAction()
        actions = [ai.getName(i) for i in range(ai.nActions)]
    except NotImplementedError:
        pass
    text = None
    editable = False
    try:
        t = acc.queryText()
        text = t.getText(0, t.characterCount)
    except NotImplementedError:
        pass
    try:
        acc.queryEditableText()
        editable = True
    except NotImplementedError:
        pass
    out = {
        "path": path,
        "role": acc.getRoleName(),
        "name": acc.name,
        "states": st,
        "actions": actions,
        "text": text,
        "editable": editable,
        "children": [],
    }
    if depth < 40:
        for i in range(acc.childCount):
            c = acc.getChildAtIndex(i)
            if c is not None:
                out["children"].append(dump(c, f"{path}.{i}" if path else str(i), depth + 1))
    return out


def main():
    cmd, name = sys.argv[1], sys.argv[2]
    app = find_app(name)
    if cmd == "dump":
        print(json.dumps(dump(app), ensure_ascii=False))
    elif cmd == "action":
        n = node_at(app, sys.argv[3])
        ai = n.queryAction()
        names = [ai.getName(i) for i in range(ai.nActions)]
        if sys.argv[4] not in names:
            raise SystemExit(json.dumps({"error": f"action {sys.argv[4]} がない", "actions": names}, ensure_ascii=False))
        print(json.dumps({"ok": ai.doAction(names.index(sys.argv[4]))}))
    elif cmd == "set-text":
        n = node_at(app, sys.argv[3])
        print(json.dumps({"ok": n.queryEditableText().setTextContents(sys.argv[4])}))


if __name__ == "__main__":
    main()
