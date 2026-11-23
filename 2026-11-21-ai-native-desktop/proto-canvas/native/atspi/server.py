#!/usr/bin/python3.12
"""ネイティブのホストの意味ツリーを AT-SPI (Linux のアクセシビリティ、D-Bus) に出すサーバ。

ブラウザでは ARIA のミラー DOM がスクリーンリーダーへの口になる。ネイティブでは、このサーバが同じ役をする。
- 標準入力 (ホスト → ここ): 1 行 1 つの JSON。{"t":"tree","windows":[...],"panel":[...]} で全体を送る
- 標準出力 (ここ → ホスト): {"t":"action","app":..,"node":..,"action":"click"|"focus"|"setValue","data":..}
- AT-SPI: アプリ「AI-native desktop」→ 窓ごとの frame → 意味ツリーのノード。ホストのパネル (人だけが答える口) も frame として出す
- スクリーンリーダーからの操作は、ブラウザの ARIA のミラーと同じく「人の操作」として shell に渡す

Ubuntu の system の Python 3.12 と python3-gi (Gio) / gir1.2-atspi-2.0 (enum の値) を使う。
"""
import json
import os
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, Gio, GLib  # noqa: E402

ROOT = "/org/a11y/atspi/accessible/root"
PREFIX = "/org/a11y/atspi/accessible/"

XML = """
<node>
  <interface name="org.a11y.atspi.Accessible">
    <property name="Name" type="s" access="read"/>
    <property name="Description" type="s" access="read"/>
    <property name="Parent" type="(so)" access="read"/>
    <property name="ChildCount" type="i" access="read"/>
    <property name="Locale" type="s" access="read"/>
    <property name="AccessibleId" type="s" access="read"/>
    <property name="HelpText" type="s" access="read"/>
    <method name="GetChildAtIndex"><arg direction="in" type="i"/><arg direction="out" type="(so)"/></method>
    <method name="GetChildren"><arg direction="out" type="a(so)"/></method>
    <method name="GetIndexInParent"><arg direction="out" type="i"/></method>
    <method name="GetRelationSet"><arg direction="out" type="a(ua(so))"/></method>
    <method name="GetRole"><arg direction="out" type="u"/></method>
    <method name="GetRoleName"><arg direction="out" type="s"/></method>
    <method name="GetLocalizedRoleName"><arg direction="out" type="s"/></method>
    <method name="GetState"><arg direction="out" type="au"/></method>
    <method name="GetAttributes"><arg direction="out" type="a{ss}"/></method>
    <method name="GetApplication"><arg direction="out" type="(so)"/></method>
    <method name="GetInterfaces"><arg direction="out" type="as"/></method>
  </interface>
  <interface name="org.a11y.atspi.Application">
    <property name="ToolkitName" type="s" access="read"/>
    <property name="Version" type="s" access="read"/>
    <property name="AtspiVersion" type="s" access="read"/>
    <property name="Id" type="i" access="readwrite"/>
    <method name="GetLocale"><arg direction="in" type="u"/><arg direction="out" type="s"/></method>
    <method name="RegisterEventListener"><arg direction="in" type="s"/></method>
    <method name="DeregisterEventListener"><arg direction="in" type="s"/></method>
  </interface>
  <interface name="org.a11y.atspi.Action">
    <property name="NActions" type="i" access="read"/>
    <method name="GetDescription"><arg direction="in" type="i"/><arg direction="out" type="s"/></method>
    <method name="GetName"><arg direction="in" type="i"/><arg direction="out" type="s"/></method>
    <method name="GetLocalizedName"><arg direction="in" type="i"/><arg direction="out" type="s"/></method>
    <method name="GetKeyBinding"><arg direction="in" type="i"/><arg direction="out" type="s"/></method>
    <method name="GetActions"><arg direction="out" type="a(sss)"/></method>
    <method name="DoAction"><arg direction="in" type="i"/><arg direction="out" type="b"/></method>
  </interface>
  <interface name="org.a11y.atspi.Component">
    <method name="Contains"><arg direction="in" type="i"/><arg direction="in" type="i"/><arg direction="in" type="u"/><arg direction="out" type="b"/></method>
    <method name="GetAccessibleAtPoint"><arg direction="in" type="i"/><arg direction="in" type="i"/><arg direction="in" type="u"/><arg direction="out" type="(so)"/></method>
    <method name="GetExtents"><arg direction="in" type="u"/><arg direction="out" type="(iiii)"/></method>
    <method name="GetPosition"><arg direction="in" type="u"/><arg direction="out" type="(ii)"/></method>
    <method name="GetSize"><arg direction="out" type="(ii)"/></method>
    <method name="GetLayer"><arg direction="out" type="u"/></method>
    <method name="GetMDIZOrder"><arg direction="out" type="n"/></method>
    <method name="GrabFocus"><arg direction="out" type="b"/></method>
    <method name="GetAlpha"><arg direction="out" type="d"/></method>
  </interface>
  <interface name="org.a11y.atspi.Text">
    <property name="CharacterCount" type="i" access="read"/>
    <property name="CaretOffset" type="i" access="read"/>
    <method name="GetText"><arg direction="in" type="i"/><arg direction="in" type="i"/><arg direction="out" type="s"/></method>
  </interface>
  <interface name="org.a11y.atspi.EditableText">
    <method name="SetTextContents"><arg direction="in" type="s"/><arg direction="out" type="b"/></method>
    <method name="InsertText"><arg direction="in" type="i"/><arg direction="in" type="s"/><arg direction="in" type="i"/><arg direction="out" type="b"/></method>
    <method name="DeleteText"><arg direction="in" type="i"/><arg direction="in" type="i"/><arg direction="out" type="b"/></method>
  </interface>
  <interface name="org.a11y.atspi.Cache">
    <method name="GetItems"><arg direction="out" type="a((so)(so)(so)iiassusau)"/></method>
  </interface>
</node>
"""
INFO = Gio.DBusNodeInfo.new_for_xml(XML)
IFACE = {i.name: i for i in INFO.interfaces}

R = Atspi.Role
ROLES = {
    "button": R.PUSH_BUTTON,
    "checkbox": R.CHECK_BOX,
    "switch": R.TOGGLE_BUTTON,
    "textbox": R.ENTRY,
    "list": R.LIST,
    "listitem": R.LIST_ITEM,
    "heading": R.HEADING,
    "paragraph": R.PARAGRAPH,
    "group": R.PANEL,
    "log": R.LOG,
    "table": R.TABLE,
    "row": R.TABLE_ROW,
    "cell": R.TABLE_CELL,
    "columnheader": R.TABLE_COLUMN_HEADER,
    "x-window": R.PANEL,
}
S = Atspi.StateType


def emit(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


class Node:
    """AT-SPI の 1 つの accessible。kind: app / frame / node / panel-button"""

    def __init__(self, key, kind, role, name, path):
        self.key, self.kind, self.role, self.name, self.path = key, kind, role, name, path
        self.parent = None
        self.children = []
        self.app = None  # shell の窓 id
        self.node = None  # 意味ツリーのノード id
        self.actions = []
        self.value = None
        self.checked = None
        self.focused = False
        self.disabled = False
        self.sensitive = False
        self.rect = None
        self.description = ""


class Server:
    def __init__(self, conn, bus_name):
        self.conn, self.bus_name = conn, bus_name
        self.paths = {}  # key → path (同じノードは同じ path を保つ)
        self.objects = {}  # path → Node
        self.registered = {}  # path → [registration id]
        self.seq = 0
        self.desktop = ("", "/org/a11y/atspi/null")
        self.app_id = 0
        self.focus_key = None
        root = Node("root", "app", R.APPLICATION, "AI-native desktop", ROOT)
        self.root = root
        self.objects[ROOT] = root
        self.register(root)
        # libatspi はアプリを見つけると、まずキャッシュ (/org/a11y/atspi/cache) を読む。中身は空 (ノードは都度問い合わせてもらう)
        self.objects["/org/a11y/atspi/cache"] = Node("cache", "cache", R.INVALID, "", "/org/a11y/atspi/cache")
        conn.register_object("/org/a11y/atspi/cache", IFACE["org.a11y.atspi.Cache"], self.on_call, None, None)

    def ref(self, n):
        return (self.bus_name, n.path) if n else ("", "/org/a11y/atspi/null")

    def path_for(self, key):
        if key not in self.paths:
            self.seq += 1
            self.paths[key] = f"{PREFIX}{self.seq}"
        return self.paths[key]

    def interfaces(self, n):
        out = ["org.a11y.atspi.Accessible", "org.a11y.atspi.Component"]
        if n.kind == "app":
            out.append("org.a11y.atspi.Application")
        if n.actions:
            out.append("org.a11y.atspi.Action")
        if n.value is not None:
            out.append("org.a11y.atspi.Text")
        if "setValue" in n.actions:
            out.append("org.a11y.atspi.EditableText")
        return out

    def register(self, n):
        ids = []
        names = ["org.a11y.atspi.Accessible", "org.a11y.atspi.Component", "org.a11y.atspi.Action", "org.a11y.atspi.Text", "org.a11y.atspi.EditableText"]
        if n.kind == "app":
            names += ["org.a11y.atspi.Application"]
        for name in names:
            ids.append(self.conn.register_object(n.path, IFACE[name], self.on_call, self.on_get, self.on_set))
        self.registered[n.path] = ids

    def unregister(self, path):
        for i in self.registered.pop(path, []):
            self.conn.unregister_object(i)

    # ---- ホストからのツリー ----
    def update(self, msg):
        old = dict(self.objects)
        keep = {ROOT}
        frames = []
        focus_key = None

        def build(key, kind, role, name, parent):
            path = self.path_for(key)
            n = Node(key, kind, role, name, path)
            n.parent = parent
            keep.add(path)
            return n

        for w in msg.get("windows", []):
            f = build(f"win/{w['app']}", "frame", R.FRAME, w["title"], self.root)
            f.rect = w.get("rect")
            f.app = w["app"]
            nodes = {x["id"]: x for x in w["nodes"]}

            def walk(nid, parent, w=w, nodes=nodes):
                nonlocal focus_key
                x = nodes.get(nid)
                if x is None:
                    return None
                n = build(f"{w['app']}/{nid}", "node", ROLES.get(x["role"], R.LABEL), x.get("label") or "", parent)
                n.app, n.node = w["app"], nid
                n.actions = [a for a in x.get("actions", []) if a in ("click", "focus", "setValue")]
                n.value = x.get("value")
                n.checked = x.get("checked")
                n.disabled = bool(x.get("disabled"))
                n.sensitive = bool(x.get("sensitive"))
                n.rect = msg.get("rects", {}).get(f"{w['app']}/{nid}")
                if x.get("sensitive"):
                    n.description = "承認が要る操作 (sensitive)"
                if w.get("focused") and w.get("focus") == nid:
                    n.focused = True
                    focus_key = n.key
                n.children = [c for c in (walk(cid, n) for cid in x.get("children", [])) if c]
                return n

            root_node = walk(w["root"], f) if w.get("root") else None
            # x-window (アプリの根) は frame と重ねず、その子を frame の子にする
            f.children = root_node.children if root_node else []
            for c in f.children:
                c.parent = f
            frames.append(f)
        panel = msg.get("panel")
        if panel is not None:
            p = build("panel", "frame", R.FRAME, "システム (ホストの UI)", self.root)
            p.description = "許可の要求・AI の提案・持ち込みに、人が答える"
            for b in panel:
                n = build(f"panel/{b['action']}", "panel-button", R.PUSH_BUTTON, b["label"], p)
                n.node = b["action"]
                n.actions = ["click"]
                n.description = b.get("context", "")
                n.rect = b.get("rect")
                p.children.append(n)
            frames.append(p)
        self.root.children = frames

        # 新しい path を登録し、消えた path を外す
        objects = {ROOT: self.root, "/org/a11y/atspi/cache": self.objects["/org/a11y/atspi/cache"]}

        def collect(n):
            for c in n.children:
                objects[c.path] = c
                collect(c)

        collect(self.root)
        for path in list(old):
            if path not in objects:
                self.unregister(path)
        for path, n in objects.items():
            if path not in self.registered and n.kind != "cache":
                self.register(n)
        changed = [p for p in objects if p not in old or (old[p].name != objects[p].name)]
        self.objects = objects
        if changed:
            self.signal(ROOT, "Object", "ChildrenChanged", "add", 0, 0, GLib.Variant("(so)", self.ref(None)))
        # フォーカスが動いたら知らせる (スクリーンリーダーが読み上げる)
        if focus_key != self.focus_key:
            self.focus_key = focus_key
            if focus_key:
                n = next((x for x in objects.values() if x.key == focus_key), None)
                if n:
                    self.signal(n.path, "Object", "StateChanged", "focused", 1, 0, GLib.Variant("i", 0))
                    self.signal(n.path, "Focus", "Focus", "", 0, 0, GLib.Variant("i", 0))

    def signal(self, path, iface, member, detail, d1, d2, any_data):
        try:
            self.conn.emit_signal(None, path, f"org.a11y.atspi.Event.{iface}", member, GLib.Variant("(siiva{sv})", (detail, d1, d2, any_data, {})))
        except GLib.Error:
            pass

    def announce(self, text):
        # AT-SPI 2.46 以降の Announcement (Orca が読み上げる)
        self.signal(ROOT, "Object", "Announcement", "", 1, 0, GLib.Variant("s", text))

    # ---- D-Bus ----
    def states(self, n):
        s = {S.ENABLED, S.SENSITIVE, S.VISIBLE, S.SHOWING} if not n.disabled else {S.VISIBLE, S.SHOWING}
        if n.kind == "frame":
            s.add(S.ACTIVE)
        if n.actions:
            s.add(S.FOCUSABLE)
        if n.focused:
            s.add(S.FOCUSED)
        if n.checked is not None:
            s.add(S.CHECKABLE)
            if n.checked:
                s.add(S.CHECKED)
        if "setValue" in n.actions:
            s.update({S.EDITABLE, S.SINGLE_LINE})
        bits = [0, 0]
        for st in s:
            v = int(st)
            bits[v // 32] |= 1 << (v % 32)
        return bits

    def on_get(self, conn, sender, path, iface, prop):
        n = self.objects.get(path)
        if n is None:
            return None
        if prop == "Name":
            return GLib.Variant("s", n.name)
        if prop == "Description":
            return GLib.Variant("s", n.description)
        if prop == "Parent":
            return GLib.Variant("(so)", self.desktop if n.kind == "app" else self.ref(n.parent))
        if prop == "ChildCount":
            return GLib.Variant("i", len(n.children))
        if prop == "Locale":
            return GLib.Variant("s", "ja_JP")
        if prop == "AccessibleId":
            return GLib.Variant("s", n.key)
        if prop == "HelpText":
            return GLib.Variant("s", "")
        if prop == "ToolkitName":
            return GLib.Variant("s", "ai-native-desktop")
        if prop == "Version":
            return GLib.Variant("s", "0.1")
        if prop == "AtspiVersion":
            return GLib.Variant("s", "2.1")
        if prop == "Id":
            return GLib.Variant("i", self.app_id)
        if prop == "NActions":
            return GLib.Variant("i", len(self.action_names(n)))
        if prop == "CharacterCount":
            return GLib.Variant("i", len(n.value or ""))
        if prop == "CaretOffset":
            return GLib.Variant("i", len(n.value or ""))
        return None

    def on_set(self, conn, sender, path, iface, prop, value):
        if prop == "Id":
            self.app_id = value.unpack()
            return True
        return False

    def action_names(self, n):
        # AT-SPI の慣習の名前 ("click" / "press" / "activate")。focus は GrabFocus で行う
        if n.kind == "panel-button" or "click" in n.actions:
            return ["click"]
        return []

    def act(self, n, action, data=None):
        if n.kind == "panel-button":
            emit({"t": "panel", "action": n.node})
        else:
            emit({"t": "action", "app": n.app, "node": n.node, "action": action, **({"data": data} if data is not None else {})})

    def on_call(self, conn, sender, path, iface, method, params, inv):
        n = self.objects.get(path)
        if n is None:
            return inv.return_dbus_error("org.a11y.atspi.Error.Defunct", "もう無い")
        a = params.unpack()
        ret = lambda sig, *v: inv.return_value(GLib.Variant(f"({sig})", v))  # noqa: E731
        if method == "GetChildAtIndex":
            i = a[0]
            return ret("(so)", self.ref(n.children[i]) if 0 <= i < len(n.children) else self.ref(None))
        if method == "GetChildren":
            return ret("a(so)", [self.ref(c) for c in n.children])
        if method == "GetIndexInParent":
            return ret("i", n.parent.children.index(n) if n.parent and n in n.parent.children else -1)
        if method == "GetRelationSet":
            return ret("a(ua(so))", [])
        if method == "GetRole":
            return ret("u", int(n.role))
        if method in ("GetRoleName", "GetLocalizedRoleName"):
            return ret("s", Atspi.role_get_name(n.role) or "")
        if method == "GetState":
            return ret("au", self.states(n))
        if method == "GetAttributes":
            attrs = {"toolkit": "ai-native-desktop"}
            if n.app:
                attrs["window"] = n.app
            if n.sensitive:
                attrs["sensitive"] = "true"
            return ret("a{ss}", attrs)
        if method == "GetApplication":
            return ret("(so)", self.ref(self.root))
        if method == "GetInterfaces":
            return ret("as", self.interfaces(n))
        if method == "GetLocale":
            return ret("s", "ja_JP")
        if method in ("RegisterEventListener", "DeregisterEventListener"):
            return inv.return_value(None)
        if method == "GetItems":
            return ret("a((so)(so)(so)iiassusau)", [])
        # Action
        names = self.action_names(n)
        if method in ("GetName", "GetLocalizedName"):
            return ret("s", names[a[0]] if 0 <= a[0] < len(names) else "")
        if method == "GetDescription":
            return ret("s", n.description)
        if method == "GetKeyBinding":
            return ret("s", "")
        if method == "GetActions":
            return ret("a(sss)", [(x, n.description, "") for x in names])
        if method == "DoAction":
            if 0 <= a[0] < len(names) and not n.disabled:
                self.act(n, "click")
                return ret("b", True)
            return ret("b", False)
        # Component
        r = n.rect or {"x": 0, "y": 0, "w": 0, "h": 0}
        if method == "GetExtents":
            return ret("(iiii)", (int(r["x"]), int(r["y"]), int(r["w"]), int(r["h"])))
        if method == "GetPosition":
            return ret("(ii)", (int(r["x"]), int(r["y"])))
        if method == "GetSize":
            return ret("(ii)", (int(r["w"]), int(r["h"])))
        if method == "Contains":
            return ret("b", r["x"] <= a[0] < r["x"] + r["w"] and r["y"] <= a[1] < r["y"] + r["h"])
        if method == "GetAccessibleAtPoint":
            return ret("(so)", self.ref(None))
        if method == "GetLayer":
            return ret("u", 3)
        if method == "GetMDIZOrder":
            return ret("n", 0)
        if method == "GetAlpha":
            return ret("d", 1.0)
        if method == "GrabFocus":
            if "focus" in n.actions:
                self.act(n, "focus")
                return ret("b", True)
            return ret("b", False)
        # Text / EditableText
        if method == "GetText":
            v = n.value or ""
            start, end = a
            return ret("s", v[start : (len(v) if end < 0 else end)])
        if method == "SetTextContents":
            if "setValue" in n.actions:
                self.act(n, "setValue", a[0])
                return ret("b", True)
            return ret("b", False)
        if method == "InsertText":
            if "setValue" in n.actions:
                v = n.value or ""
                pos = max(0, min(a[0], len(v)))
                self.act(n, "setValue", v[:pos] + a[1] + v[pos:])
                return ret("b", True)
            return ret("b", False)
        if method == "DeleteText":
            if "setValue" in n.actions:
                v = n.value or ""
                self.act(n, "setValue", v[: a[0]] + v[a[1] :])
                return ret("b", True)
            return ret("b", False)
        return inv.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method)


def a11y_bus():
    session = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    addr = session.call_sync("org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress", None, GLib.VariantType("(s)"), 0, 5000, None).unpack()[0]
    return Gio.DBusConnection.new_for_address_sync(addr, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)


def main():
    try:
        conn = a11y_bus()
    except GLib.Error as e:
        emit({"t": "error", "message": f"AT-SPI のバスにつなげない: {e.message}"})
        return 1
    server = Server(conn, conn.get_unique_name())
    # レジストリに、このアプリの根を埋め込む (デスクトップの子になる)
    try:
        server.desktop = conn.call_sync(
            "org.a11y.atspi.Registry", ROOT, "org.a11y.atspi.Socket", "Embed", GLib.Variant("((so))", ((server.bus_name, ROOT),)), GLib.VariantType("((so))"), 0, 5000, None
        ).unpack()[0]
    except GLib.Error as e:
        emit({"t": "error", "message": f"レジストリに登録できない: {e.message}"})
        return 1
    emit({"t": "ready", "bus": server.bus_name})

    loop = GLib.MainLoop()
    buf = b""

    def on_stdin(fd, cond):
        nonlocal buf
        chunk = os.read(fd, 1 << 16)
        if not chunk:
            loop.quit()
            return False
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            if msg.get("t") == "tree":
                server.update(msg)
            elif msg.get("t") == "announce":
                server.announce(msg.get("text", ""))
        return True

    GLib.io_add_watch(sys.stdin.fileno(), GLib.IO_IN | GLib.IO_HUP, on_stdin)
    loop.run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
