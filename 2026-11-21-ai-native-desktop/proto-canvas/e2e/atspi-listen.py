#!/usr/bin/python3.12
"""AT-SPI のイベントを聞いて、1 行 1 つの JSON で出す (e2e/native-a11y.ts が使う)。スクリーンリーダーが受けるのと同じイベント"""
import json
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib  # noqa: E402


def on_event(e):
    try:
        name = e.source.get_name() if e.source else ""
    except GLib.Error:
        name = ""
    any_data = e.any_data if isinstance(e.any_data, str) else None
    print(json.dumps({"type": e.type, "name": name, "detail1": e.detail1, "text": any_data}, ensure_ascii=False), flush=True)


listener = Atspi.EventListener.new(on_event)
for t in ["object:state-changed:focused", "object:state-changed:checked", "object:children-changed", "object:announcement", "focus:"]:
    listener.register(t)
print(json.dumps({"type": "listening"}), flush=True)
Atspi.event_main()
