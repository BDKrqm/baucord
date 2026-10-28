#!/usr/bin/env python3
"""The smallest bot: answers `!ping` with `pong` and greets whoever
joins. `pip install websockets`; a baucord-gateway on 127.0.0.1:8765."""
import json
import os

from websockets.sync.client import connect

GATEWAY = os.environ.get("BAUCORD_GATEWAY", "ws://127.0.0.1:8765/")


def action(ws, t, **d):
    ws.send(json.dumps({"op": "action", "t": t, "d": d}))


with connect(GATEWAY) as ws:
    hello = json.loads(ws.recv())
    print("gateway", hello["d"]["gateway"], "connected:", hello["d"]["connected"])
    while True:
        f = json.loads(ws.recv())
        if f["op"] == "error":
            print("gateway says:", f["d"])
        if f["op"] != "dispatch":
            continue
        t, d = f["t"], f["d"]
        if t == "READY":
            print("on", d["server_name"], "with", [m["name"] for m in d["members"]])
        elif t == "MESSAGE_CREATE" and not d["own"] and d["text"].strip() == "!ping":
            action(ws, "CREATE_MESSAGE", text="pong")
        elif t == "MEMBER_ADD":
            action(ws, "CREATE_MESSAGE", text="hi " + d["name"])
