#!/usr/bin/python3.12
"""既存アプリの代役: 普通の GTK3 アプリ。アクセシビリティは GTK 自身の AT-SPI 実装がそのまま出す。"""
import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk  # noqa: E402


class LegacyTodo(Gtk.Window):
    def __init__(self):
        super().__init__(title="Legacy Todo")
        self.set_default_size(420, 320)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, margin=12)
        self.add(box)
        heading = Gtk.Label(label="やること (GTK)")
        heading.get_accessible().set_role(gi.repository.Atk.Role.HEADING) if hasattr(gi.repository, "Atk") else None
        box.pack_start(heading, False, False, 0)
        self.items = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        box.pack_start(self.items, False, False, 0)
        for text, done in [("牛乳を買う", False), ("請求書を送る", True)]:
            self.add_item(text, done)
        row = Gtk.Box(spacing=8)
        self.entry = Gtk.Entry(placeholder_text="新しいタスク")
        self.entry.get_accessible().set_name("新しいタスク")
        add = Gtk.Button(label="追加")
        add.connect("clicked", self.on_add)
        row.pack_start(self.entry, True, True, 0)
        row.pack_start(add, False, False, 0)
        box.pack_start(row, False, False, 0)
        sw_row = Gtk.Box(spacing=8)
        self.switch = Gtk.Switch()
        self.switch.get_accessible().set_name("通知")
        sw_row.pack_start(Gtk.Label(label="通知"), False, False, 0)
        sw_row.pack_start(self.switch, False, False, 0)
        box.pack_start(sw_row, False, False, 0)
        self.status = Gtk.Label(label="変更なし")
        box.pack_start(self.status, False, False, 0)
        self.connect("destroy", Gtk.main_quit)

    def add_item(self, text, done=False):
        cb = Gtk.CheckButton(label=text)
        cb.set_active(done)
        cb.connect("toggled", lambda w: self.status.set_text(f"{w.get_label()}: {'完了' if w.get_active() else '未完了'}"))
        self.items.pack_start(cb, False, False, 0)
        cb.show()

    def on_add(self, _):
        text = self.entry.get_text().strip()
        if text:
            self.add_item(text)
            self.entry.set_text("")
            self.status.set_text(f"追加: {text}")


if __name__ == "__main__":
    import gi.repository  # noqa: F401

    try:
        gi.require_version("Atk", "1.0")
        from gi.repository import Atk  # noqa: F401
    except Exception:
        pass
    win = LegacyTodo()
    win.show_all()
    Gtk.main()
