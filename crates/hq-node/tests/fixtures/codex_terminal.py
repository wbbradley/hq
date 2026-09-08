#!/usr/bin/python3
import json
import os
import sys
import time
import queue
import threading

thread_id = "hq-test-thread"
counter_path = os.path.join(os.path.dirname(__file__), "turn-number")
turn_number = int(open(counter_path).read()) if os.path.exists(counter_path) else 0
interrupt_fixture = os.path.exists(os.path.join(os.path.dirname(__file__), "wait-for-interrupt"))
lines = sys.stdin
if interrupt_fixture:
    wire_lines = queue.Queue()

    def read_wire():
        for incoming in sys.stdin:
            wire_lines.put(incoming)
        wire_lines.put(None)

    threading.Thread(target=read_wire, daemon=True).start()
    lines = iter(wire_lines.get, None)

for line in lines:
    with open(os.path.join(os.path.dirname(__file__), "calls.log"), "a") as log:
        log.write(line)
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if request_id is None:
        continue
    if method == "initialize":
        result = {}
    elif method == "thread/start":
        result = {"thread": {"id": thread_id, "turns": []}}
    elif method == "thread/resume":
        thread_id = message.get("params", {}).get("threadId", thread_id)
        result = {"thread": {"id": thread_id, "turns": []}}
    elif method == "turn/start":
        turn_number += 1
        with open(counter_path, "w") as counter:
            counter.write(str(turn_number))
        turn_id = f"hq-test-turn-{turn_number}"
        turn = {"id": turn_id, "status": "inProgress", "items": []}
        print(json.dumps({"method": "turn/started", "params": {"threadId": thread_id, "turn": turn}}), flush=True)
        print(json.dumps({"id": request_id, "result": {"turn": turn}}), flush=True)
        if os.path.exists(os.path.join(os.path.dirname(__file__), "progress-flood")):
            with open(os.path.join(os.path.dirname(__file__), "flood-started"), "w") as marker:
                marker.write("started")
            for sequence in range(2000):
                if sequence % 100 == 0:
                    with open(os.path.join(os.path.dirname(__file__), "flood-position"), "w") as marker:
                        marker.write(str(sequence))
                progress = {"method": "item/commandExecution/outputDelta", "params": {"threadId": thread_id, "turnId": turn_id, "itemId": f"command-{turn_number}", "delta": f"progress-{sequence}"}}
                print(json.dumps(progress), flush=True)
                if sequence % 200 == 199:
                    time.sleep(0.1)
            with open(os.path.join(os.path.dirname(__file__), "flood-finished"), "w") as marker:
                marker.write("finished")
        interrupt_fixture = os.path.exists(os.path.join(os.path.dirname(__file__), "wait-for-interrupt"))
        if turn_number == 1 and interrupt_fixture:
            with open(os.path.join(os.path.dirname(__file__), "interrupt-waiting"), "w") as marker:
                marker.write("waiting")
        if os.path.exists(os.path.join(os.path.dirname(__file__), "request-approval")) and (not interrupt_fixture or turn_number == 1):
            approval_id = 900 + turn_number
            approval = {"id": approval_id, "method": "item/commandExecution/requestApproval", "params": {"threadId": thread_id, "turnId": turn_id, "itemId": f"command-{turn_number}", "command": "cargo test", "cwd": os.getcwd(), "reason": "Run the test command?"}}
            print(json.dumps(approval), flush=True)
            for answer_line in lines:
                with open(os.path.join(os.path.dirname(__file__), "calls.log"), "a") as log:
                    log.write(answer_line)
                answer = json.loads(answer_line)
                if answer.get("id") == approval_id:
                    break
        if os.path.exists(os.path.join(os.path.dirname(__file__), "history-pages")):
            for index in range(120):
                label = "HISTORY_START" if index == 0 else "HISTORY_END" if index == 119 else f"history-entry-{index:03}"
                item = {"type": "commandExecution", "id": f"history-{index}", "status": "completed", "command": f"echo {label}", "aggregatedOutput": label, "exitCode": 0}
                print(json.dumps({"method": "item/completed", "params": {"threadId": thread_id, "turnId": turn_id, "item": item}}), flush=True)
        if turn_number == 1 and os.path.exists(os.path.join(os.path.dirname(__file__), "wait-for-interrupt")):
            with open(os.path.join(os.path.dirname(__file__), "interrupt-waiting"), "w") as marker:
                marker.write("waiting")
            for control_line in lines:
                with open(os.path.join(os.path.dirname(__file__), "calls.log"), "a") as log:
                    log.write(control_line)
                control = json.loads(control_line)
                if control.get("method") == "thread/read":
                    print(json.dumps({"id": control["id"], "result": {"thread": {"id": thread_id, "turns": [turn]}}}), flush=True)
                elif control.get("method") == "turn/interrupt":
                    assert control["params"]["threadId"] == thread_id
                    assert control["params"]["turnId"] == turn_id
                    with open(os.path.join(os.path.dirname(__file__), "interrupt-received"), "w") as marker:
                        marker.write(turn_id)
                    print(json.dumps({"id": control["id"], "result": {}}), flush=True)
                    hold_terminal = os.path.join(os.path.dirname(__file__), "hold-interrupted-terminal")
                    release_terminal = os.path.join(os.path.dirname(__file__), "release-interrupted-terminal")
                    while os.path.exists(hold_terminal) and not os.path.exists(release_terminal):
                        try:
                            pending_line = wire_lines.get(timeout=0.02)
                        except queue.Empty:
                            continue
                        assert pending_line is not None, "provider closed before terminal release"
                        with open(os.path.join(os.path.dirname(__file__), "calls.log"), "a") as log:
                            log.write(pending_line)
                        pending = json.loads(pending_line)
                        assert pending.get("method") == "thread/read", "new work reached provider before old terminal"
                        print(json.dumps({"id": pending["id"], "result": {"thread": {"id": thread_id, "turns": [turn]}}}), flush=True)
                    print(json.dumps({"method": "turn/completed", "params": {"threadId": thread_id, "turn": {"id": turn_id, "status": "interrupted", "items": []}}}), flush=True)
                    break
            continue
        completion_gate = os.path.join(os.path.dirname(__file__), f"completion-gate-{turn_number}")
        if os.path.exists(completion_gate):
            with open(completion_gate, "rb", buffering=0) as gate:
                gate.read(1)
        else:
            time.sleep(0.25)
        item = {"type": "agentMessage", "id": f"answer-{turn_number}", "text": f"finished-turn-{turn_number}", "phase": "final_answer"}
        print(json.dumps({"method": "item/completed", "params": {"threadId": thread_id, "turnId": turn_id, "item": item}}), flush=True)
        completed = {"id": turn_id, "status": "completed", "items": [item]}
        print(json.dumps({"method": "turn/completed", "params": {"threadId": thread_id, "turn": completed}}), flush=True)
        continue
    elif method == "thread/read":
        result = {"thread": {"id": thread_id, "turns": []}}
    elif method in ("turn/interrupt", "turn/steer"):
        result = {"turnId": "hq-test-turn"}
    else:
        continue
    print(json.dumps({"id": request_id, "result": result}), flush=True)
