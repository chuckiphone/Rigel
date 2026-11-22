#!/usr/bin/env python3
"""A minimal Rigel plugin: newline-delimited JSON on stdin/stdout, protocol 1.

It never touches the filesystem itself. `read_note` asks the host for `fs.read`, and the
host answers from the manifest's capability grant, so this file works unchanged with a
grant, without one, and outside the plugin directory.
"""

import json
import sys


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def ask(capability, **fields):
    send({"type": "capability", "id": 2, "capability": capability, **fields})
    line = sys.stdin.readline()
    if not line:
        return {"ok": False, "error": {"code": "EOF", "message": "host closed the pipe"}}
    return json.loads(line)


def main():
    for line in sys.stdin:
        message = json.loads(line)
        kind = message.get("type")

        if kind == "hello":
            send({"type": "ready", "protocol": 1})
            continue

        if kind != "call":
            continue

        tool = message.get("tool")
        arguments = message.get("arguments") or {}

        if tool == "hello":
            name = arguments.get("name") or "world"
            send({
                "type": "result",
                "id": message["id"],
                "ok": True,
                "content": f"Hello, {name}. A Rigel plugin answered in a child process.",
            })
        elif tool == "read_note":
            path = arguments.get("path") or "README.txt"
            answer = ask("fs.read", path=path)
            send({
                "type": "result",
                "id": message["id"],
                "ok": True,
                "content": answer,
            })
        else:
            send({
                "type": "result",
                "id": message["id"],
                "ok": False,
                "error": f"no tool named {tool}",
            })


if __name__ == "__main__":
    main()
