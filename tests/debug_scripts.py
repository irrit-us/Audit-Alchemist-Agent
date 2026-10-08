"""Run with python -B tests/debug_scripts.py; optional native tools are reported as skips."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
NODE_SCRIPT = ROOT / "skills/node-inspector/scripts/inspect.mjs"
RAW = ROOT / "skills/foundry-debugging/scripts/RawDebug.sol"
TRACE = ROOT / "skills/foundry-debugging/scripts/forge_trace.py"
SPEC = importlib.util.spec_from_file_location("tube_probe", ROOT / "skills/pwntools-debugging/scripts/tube_probe.py")
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


class Tube:
    def __init__(self, chunks):
        self.chunks, self.sent = iter(chunks), []

    def recv(self, count, timeout):
        assert timeout > 0
        chunk = next(self.chunks, None)
        if chunk is None:
            raise EOFError()
        assert len(chunk) <= count
        return chunk

    def send(self, data):
        self.sent.append(data)

    def shutdown(self, direction):
        assert direction == "send"


class TubeTests(unittest.TestCase):
    def test_missing_prompt_never_sends(self):
        tube = Tube([b"wrong", b""])
        result = PROBE.probe(tube, b"payload", b"> ", True, 1, 100)
        self.assertEqual(result["state"], "prompt_timeout")
        self.assertEqual(tube.sent, [])

    def test_fragmented_prompt_binary_payload_and_eof(self):
        tube = Tube([b">", b" ", b"\x00\xff"])
        result = PROBE.probe(tube, b"\x00\x80", b"> ", True, 1, 100)
        self.assertEqual(tube.sent, [b"\x00\x80\n"])
        self.assertEqual(result["output_hex"], "3e2000ff")
        self.assertEqual(result["state"], "eof")

    def test_receive_limit_is_explicit(self):
        result = PROBE.probe(Tube([b"abcd"]), b"x", None, False, 1, 4)
        self.assertEqual(result["state"], "output_limit")
        self.assertEqual(result["output_hex"], "61626364")

    @unittest.skipUnless(importlib.util.find_spec("pwnlib"), "pwntools not installed")
    def test_real_process_echo(self):
        result = subprocess.run([sys.executable, str(ROOT / "skills/pwntools-debugging/scripts/tube_probe.py"),
            "--payload-hex", "00ff", "--timeout", "5", "--", sys.executable, "-c",
            "import sys; sys.stdout.buffer.write(sys.stdin.buffer.read())"], capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["output_hex"], "00ff")


class GdbScriptTests(unittest.TestCase):
    def test_embedded_python_syntax(self):
        source = (ROOT / "skills/gdb-debugging/scripts/capture.gdb").read_text(encoding="utf-8")
        python = source.split("\npython\n", 1)[1].rsplit("\nend", 1)[0]
        compile(python, "capture.gdb", "exec")

    @unittest.skipUnless(shutil.which("gdb"), "GDB not installed")
    def test_real_inferior_exit_and_signal(self):
        script = ROOT / "skills/gdb-debugging/scripts/capture.gdb"
        for code, expected in [("import sys; sys.exit(7)", "exited"),
                               ("import os, signal; os.kill(os.getpid(), signal.SIGSEGV)", "signal")]:
            result = subprocess.run(["gdb", "-q", "-nx", "-batch", "-x", str(script),
                "--args", sys.executable, "-c", code], capture_output=True, text=True, timeout=20)
            self.assertNotEqual(result.returncode, 0)
            records = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{"outcome":')]
            self.assertTrue(records, result.stdout + result.stderr)
            self.assertEqual(records[-1]["outcome"], expected, result.stdout + result.stderr)
            if expected == "exited":
                self.assertEqual(records[-1]["exit_code"], 7)
            else:
                self.assertEqual(records[-1]["signal"], "SIGSEGV")


@unittest.skipUnless(shutil.which("node"), "Node not installed")
class NodeTests(unittest.TestCase):
    def run_target(self, source, *options):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "target.cjs"
            target.write_text(source, encoding="utf-8")
            options = [str(target) + ":2" if option == "BREAK" else option for option in options]
            result = subprocess.run(["node", str(NODE_SCRIPT), *options, "--", str(target)],
                capture_output=True, text=True, encoding="utf-8", timeout=15)
            records = [json.loads(line) for line in result.stderr.splitlines() if line.startswith('{"type":')]
            return result, records

    def test_breakpoint_evaluates_target_frame(self):
        result, records = self.run_target("const value = 37;\nconsole.log('target-done', value);\n",
            "--break", "BREAK", "--expression", "value")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("target-done 37", result.stdout)
        self.assertTrue(any(r["type"] == "evaluation" and r["value"]["value"] == 37 for r in records), records)
        self.assertEqual(records[-1]["type"], "target_exit")

    def test_exception_is_captured_and_exit_preserved(self):
        result, records = self.run_target("throw new Error('fixture-crash');\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(any(r["type"] == "paused" and r["reason"] == "exception" for r in records), records)
        self.assertEqual(records[-1]["code"], 1)

    def test_busy_target_is_terminated_at_deadline(self):
        result, records = self.run_target("while (true) {}\n", "--timeout-ms", "1000")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(any('deadline' in r.get("error", "") for r in records), records)


@unittest.skipUnless(os.environ.get("AUDIT_TEST_FOUNDRY") == "1" and shutil.which("forge"),
    "set AUDIT_TEST_FOUNDRY=1 to compile Foundry fixtures (may download solc)")
class FoundryTests(unittest.TestCase):
    def test_raw_fallback_on_old_and_current_compilers(self):
        for version in ["0.6.12", "0.8.26"]:
            with self.subTest(solc=version), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "test").mkdir()
                (root / "foundry.toml").write_text('[profile.default]\nsolc = "' + version + '"\n', encoding="utf-8")
                shutil.copyfile(RAW, root / "test/RawDebug.sol")
                (root / "test/Probe.t.sol").write_text('''pragma solidity >=0.6.0 <0.9.0;
import "./RawDebug.sol";
contract RawCapabilityTest {
    function testRawCapability() public {
        RawDebug.requireVm();
        RawDebug.callVm(abi.encodeWithSignature("deal(address,uint256)", address(0xBEEF), uint256(123)));
        require(address(0xBEEF).balance == 123, "deal did not run");
        RawDebug.logString("raw-debug-compatible");
        RawDebug.logUint(123);
        RawDebug.logBytes(hex"deadbeef");
        (bool ok, ) = address(this).call(abi.encodeWithSignature("unknownSelector()"));
        require(!ok, "missing selector silently succeeded");
    }
    function unknownSelector() public { RawDebug.callVm(abi.encodeWithSignature("notARealCheatcode()")); }
}''', encoding="utf-8")
                result = subprocess.run([sys.executable, str(TRACE), "--root", str(root),
                    "--match-contract", "RawCapabilityTest", "--match-test", "testRawCapability"],
                    capture_output=True, text=True, encoding="utf-8", timeout=120)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                for text in ["raw-debug-compatible", "123", "deadbeef", "1 passed"]:
                    self.assertIn(text, result.stdout)

    @unittest.skipUnless(os.environ.get("AUDIT_TEST_FORGE_STD"), "set AUDIT_TEST_FORGE_STD to installed forge-std")
    def test_typed_cheatcodes_with_compatible_forge_std(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "test").mkdir()
            library = Path(os.environ["AUDIT_TEST_FORGE_STD"]).resolve().as_posix() + "/src/"
            (root / "foundry.toml").write_text('[profile.default]\nsolc = "0.8.26"\nremappings = [' +
                json.dumps("forge-std/=" + library) + ']\n', encoding="utf-8")
            shutil.copyfile(ROOT / "skills/foundry-debugging/scripts/TypedDebug.t.sol", root / "test/TypedDebug.t.sol")
            result = subprocess.run(["forge", "test", "--root", str(root), "-vv"],
                capture_output=True, text=True, encoding="utf-8", timeout=120)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("typed-debug-compatible", result.stdout)


if __name__ == "__main__":
    if os.environ.get("AUDIT_REQUIRE_DEBUG_TOOLS") == "1":
        missing = [tool for tool in ("node", "gdb") if not shutil.which(tool)]
        if not importlib.util.find_spec("pwnlib"):
            missing.append("pwntools")
        if missing:
            raise SystemExit("Required debug test dependencies missing: " + ", ".join(missing))
    unittest.main()
