#!/usr/bin/env python3
"""A music bot: `!play <url>` joins the asker's channel and plays the
URL (yt-dlp → ffmpeg → the gateway's voice sink); `!stop` stops.

Needs: `pip install websockets`, `yt-dlp` and `ffmpeg` on PATH, and a
baucord-gateway on 127.0.0.1:8765 whose bot has JOIN_CHANNELS and SPEAK
(a member does). The gateway paces the audio: this script pushes
ffmpeg's output as fast as the socket takes it, 100 ms at a time."""
import json
import os
import shlex
import subprocess
import threading

from websockets.sync.client import connect

GATEWAY = os.environ.get("BAUCORD_GATEWAY", "ws://127.0.0.1:8765/")
PIECE = 4800 * 4  # 100 ms of f32 mono at 48 kHz


class Player:
    """One ffmpeg at a time, its stdout streamed to the gateway."""

    def __init__(self, ws):
        self.ws = ws
        self.proc = None
        self.lock = threading.Lock()

    def play(self, url):
        self.stop()
        cmd = "yt-dlp -q -o - %s | ffmpeg -loglevel error -i - -f f32le -ar 48000 -ac 1 -" % shlex.quote(url)
        with self.lock:
            self.proc = subprocess.Popen(cmd, shell=True, stdout=subprocess.PIPE)
            proc = self.proc
        threading.Thread(target=self._pump, args=(proc,), daemon=True).start()

    def _pump(self, proc):
        while True:
            piece = proc.stdout.read(PIECE)
            if not piece:
                break
            with self.lock:
                if self.proc is not proc:
                    break
            self.ws.send(piece)  # blocks when ten seconds are queued: the pacing
        proc.wait()

    def stop(self):
        with self.lock:
            proc, self.proc = self.proc, None
        if proc:
            proc.kill()
            self.ws.send(json.dumps({"op": "voice", "t": "STOP"}))


def action(ws, t, **d):
    ws.send(json.dumps({"op": "action", "t": t, "d": d}))


with connect(GATEWAY) as ws:
    hello = json.loads(ws.recv())
    channels = {}  # client_id -> channel_id, from READY and VOICE_STATE
    ready = hello["d"]["ready"]
    if ready:
        channels = {m["client_id"]: m["channel_id"] for m in ready["members"]}
    player = Player(ws)
    while True:
        f = json.loads(ws.recv())
        if f["op"] == "error":
            print("gateway says:", f["d"])
            continue
        if f["op"] != "dispatch":
            continue
        t, d = f["t"], f["d"]
        if t == "READY":
            channels = {m["client_id"]: m["channel_id"] for m in d["members"]}
        elif t in ("VOICE_STATE", "MEMBER_ADD"):
            channels[d["client_id"]] = d["channel_id"]
        elif t == "MEMBER_REMOVE":
            channels.pop(d["client_id"], None)
        elif t == "MESSAGE_CREATE" and not d["own"]:
            words = d["text"].split(maxsplit=1)
            if words[:1] == ["!play"] and len(words) == 2:
                channel = channels.get(d["client_id"])
                if channel is None:
                    action(ws, "CREATE_MESSAGE", text="join a voice channel first, %s" % d["name"])
                    continue
                action(ws, "JOIN_CHANNEL", channel_id=channel)
                action(ws, "CREATE_MESSAGE", text="playing %s for %s" % (words[1], d["name"]))
                player.play(words[1])
            elif words[:1] == ["!stop"]:
                player.stop()
                action(ws, "CREATE_MESSAGE", text="stopped")
