#!/usr/bin/env python3
"""Opt-in PipeWire/PulseAudio test. All generated audio stays on virtual devices."""
import array
import json
import math
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import threading
import time

BINARY = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/oto-oto").resolve())
PREFIX = "oto_oto_mix_"
PACTL = shutil.which("pactl")


def pactl(*args):
    return subprocess.check_output([PACTL, *args], text=True, timeout=10).strip()


def listing(kind):
    return json.loads(pactl("--format=json", "list", kind))


def defaults():
    info = json.loads(pactl("--format=json", "info"))
    return info["default_sink_name"], info["default_source_name"]


def assert_clean():
    assert not any(i["name"].startswith(PREFIX) for i in listing("sinks")), "Virtual sinks leaked"
    assert not any("oto.session=oto-session-" in line for line in pactl("list", "short", "modules").splitlines()), "Mixer modules leaked"


class Worker:
    def __init__(self, env=None):
        self.process = subprocess.Popen([BINARY, "--mix-worker"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env, start_new_session=True)

    def ask(self, action, allow_error=False):
        self.process.stdin.write(json.dumps(action) + "\n")
        self.process.stdin.flush()
        if not select.select([self.process.stdout], [], [], 25)[0]:
            raise TimeoutError("Mixer worker did not respond")
        reply = json.loads(self.process.stdout.readline())
        if not allow_error:
            assert reply["error"] is None, reply["error"]
        return reply

    def close(self):
        if not self.process.stdin.closed:
            self.process.stdin.close()
        self.process.wait(timeout=25)
        assert self.process.returncode == 0


def level(worker, channel, bus, volume=70, muted=False):
    worker.ask({"Level": {"channel": channel, "bus": bus, "level": {"volume": volume, "muted": muted}}})


def capture(source):
    process = subprocess.Popen(["parec", "--raw", "--format=float32le", "--rate=48000", "--channels=2", "--latency-msec=20", "--device=" + source], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        process.communicate(timeout=0.65)
        raise AssertionError("Capture exited early")
    except subprocess.TimeoutExpired:
        process.terminate()
        data, error = process.communicate(timeout=5)
    assert len(data) > 19200 * 4, (len(data), error)
    samples = array.array("f", data[-19200 * 4:])
    if sys.byteorder != "little":
        samples.byteswap()
    return math.sqrt(sum(v * v for v in samples) / len(samples))


def main():
    for tool in ["pactl", "pacat", "parec"]:
        assert shutil.which(tool), f"Missing {tool}"
    assert_clean()
    before = defaults()
    modules = []
    worker = None
    player = None
    feed = None
    stop = threading.Event()
    sink = f"oto_audio_check_{os.getpid()}"
    try:
        props = json.dumps('device.description="Oto audio test" node.dont-fallback=true priority.session=0')
        modules.append(pactl("load-module", "module-null-sink", "sink_name=" + sink, "sink_properties=" + props))
        test_sink_id = next(i["index"] for i in listing("sinks") if i["name"] == sink)
        worker = Worker()
        start = {"Start": {"monitor": sink, "microphone": None}}
        reply = worker.ask(start)
        assert reply["state"]["active"]
        assert defaults() == before, "Starting the mixer changed system defaults"

        other = Worker()
        try:
            assert other.ask(start, allow_error=True)["error"], "Second mixer should be refused"
        finally:
            other.close()

        player = subprocess.Popen(["pacat", "--playback", "--raw", "--format=float32le", "--rate=48000", "--channels=2", "--device=" + sink, "--client-name=OtoAudioTest", "--latency-msec=20"], stdin=subprocess.PIPE, stderr=subprocess.DEVNULL)

        def tone():
            phase = 0
            try:
                while not stop.is_set():
                    samples = array.array("f")
                    for _ in range(960):
                        value = 0.1 * math.sin(phase * 2 * math.pi * 440 / 48000)
                        phase += 1
                        samples.extend([value, value])
                    if sys.byteorder != "little":
                        samples.byteswap()
                    player.stdin.write(samples.tobytes())
                    player.stdin.flush()
            except (BrokenPipeError, ValueError):
                pass

        feed = threading.Thread(target=tone, daemon=True)
        feed.start()
        deadline = time.monotonic() + 5
        stream = None
        while time.monotonic() < deadline:
            stream = next((i for i in listing("sink-inputs") if i["properties"].get("application.name") == "OtoAudioTest"), None)
            if stream and stream["sink"] == test_sink_id:
                break
            time.sleep(0.03)
        assert stream, "Test app did not appear"
        original_sink = test_sink_id
        worker.ask({"Assign": {"stream": stream["index"], "channel": 0}})
        base_stream = capture(PREFIX + "stream")
        base_listen = capture(sink + ".monitor")
        assert min(base_stream, base_listen) > 0.002, (base_stream, base_listen)

        level(worker, 0, "Listen", volume=10)
        quiet_listen = capture(sink + ".monitor")
        same_stream = capture(PREFIX + "stream")
        assert quiet_listen < base_listen * 0.1, (base_listen, quiet_listen)
        assert 0.8 < same_stream / base_stream < 1.2, (base_stream, same_stream)

        level(worker, 0, "Listen")
        level(worker, 0, "Stream", muted=True)
        silent_stream = capture(PREFIX + "stream")
        audible_listen = capture(sink + ".monitor")
        assert silent_stream < 0.0001, silent_stream
        assert audible_listen > base_listen * 0.8, audible_listen

        level(worker, 0, "Stream")
        level(worker, None, "Stream", muted=True)
        assert capture(PREFIX + "stream") < 0.0001, "Stream master mute failed"
        assert capture(sink + ".monitor") > base_listen * 0.8, "Stream master affected Listen"

        # Closing the pipe simulates the UI exiting or crashing; the helper must restore routes.
        worker.close()
        worker = None
        restored = next(i for i in listing("sink-inputs") if i["index"] == stream["index"])
        assert restored["sink"] == original_sink, ("App was not restored to its original output", original_sink, restored["sink"], [(i["index"], i["name"]) for i in listing("sinks")])
        assert_clean()
        assert defaults() == before
        print("PASS: independent Listen/Stream gains, channel mute, master mute, exclusivity, route restoration, and EOF cleanup")

        # Use a remapped virtual microphone, fed by the test player, without opening hardware.
        mic = sink + "_mic"
        modules.append(pactl("load-module", "module-remap-source", "master=" + sink + ".monitor", "source_name=" + mic,
                             "source_properties=" + json.dumps('device.description="Oto test mic" node.dont-fallback=true priority.session=0')))
        worker = Worker()
        worker.ask({"Start": {"monitor": sink, "microphone": mic}})
        assert capture(PREFIX + "stream") > 0.002, "Microphone was not mixed into Stream"
        assert capture(PREFIX + "listen_bus.monitor") < 0.0001, "Microphone monitoring should start muted"
        level(worker, 4, "Stream", muted=True)
        assert capture(PREFIX + "stream") < 0.0001, "Microphone Stream mute failed"
        connection = next(i for i in listing("sink-inputs") if i["properties"].get("application.id") == "io.oto-oto.mixer")
        pactl("unload-module", str(connection["owner_module"]))
        unhealthy = worker.ask("Inspect", allow_error=True)
        assert unhealthy["error"] and not unhealthy["state"]["active"], "Missing mix connection was not reported"
        worker.close()
        worker = None
        assert_clean()
        print("PASS: virtual microphone capture, safe monitoring default, mic mute, and lost-connection cleanup")

        # Fail the fourth module load to verify rollback of a partially created graph.
        with tempfile.TemporaryDirectory(prefix="oto-audio-failure-") as directory:
            wrapper = Path(directory, "pactl")
            wrapper.write_text("#!/usr/bin/env python3\nimport os,sys,pathlib\np=pathlib.Path(os.environ['OTO_LOAD_COUNT'])\nif len(sys.argv)>1 and sys.argv[1]=='load-module':\n n=int(p.read_text())+1 if p.exists() else 1\n p.write_text(str(n))\n if n==4:\n  print('injected module failure',file=sys.stderr)\n  sys.exit(1)\nos.execv(os.environ['OTO_REAL_PACTL'],[os.environ['OTO_REAL_PACTL'],*sys.argv[1:]])\n")
            wrapper.chmod(0o755)
            env = dict(os.environ, PATH=directory + os.pathsep + os.environ["PATH"], OTO_REAL_PACTL=PACTL, OTO_LOAD_COUNT=str(Path(directory, "count")))
            failed = Worker(env)
            try:
                response = failed.ask(start, allow_error=True)
                assert response["error"] and not response["state"]["active"]
                assert_clean()
            finally:
                failed.close()
        print("PASS: partial-start rollback; defaults preserved")
    finally:
        if worker:
            worker.close()
        if player:
            stop.set()
            player.terminate()
            player.wait(timeout=5)
            if feed:
                feed.join(timeout=5)
            try:
                player.stdin.close()
            except BrokenPipeError:
                pass
        for module in reversed(modules):
            subprocess.run([PACTL, "unload-module", module], check=True, timeout=10)
        assert defaults() == before, "System defaults changed during audio verification"


if __name__ == "__main__":
    main()
