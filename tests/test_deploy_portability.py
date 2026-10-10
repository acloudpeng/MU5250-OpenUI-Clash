"""Deployment tool behaviour that must hold on Windows as well as POSIX hosts.

CI runs this module on Linux and on a Windows runner (see checks.yml), where run()
takes its thread-based path and a script stands in for ssh.
"""
import hashlib
import importlib.util
import os
from pathlib import Path
import shutil
import socket
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
IDENTITY = {'model': 'ZTE MU5250', 'firmware': 'test', 'fingerprint': 'aabb'}


def load_deploy():
    sys.path.insert(0, str(ROOT / 'scripts'))
    try:
        spec = importlib.util.spec_from_file_location('deploy_components', ROOT / 'scripts/deploy-components.py')
        module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        return module
    finally:
        sys.path.remove(str(ROOT / 'scripts'))


def git_bash():
    # On Windows, Git Bash rather than a WSL launcher that may also be on PATH.
    candidate = Path(os.environ.get('ProgramFiles', 'C:/Program Files')) / 'Git/bin/bash.exe'
    return str(candidate) if os.name == 'nt' and candidate.is_file() else shutil.which('bash')


def aarch64(extra=b''):
    header = bytearray(64); header[:6] = b'\x7fELF\x02\x01'; header[18:20] = (183).to_bytes(2, 'little')
    return bytes(header) + extra


class FakeModem:
    """Answers deploy() as a modem would; records commands and refuses writes."""
    def __init__(self, startup='', installed='', screen_active=False):
        self.startup, self.installed, self.screen_active = startup, installed, screen_active
        self.serial, self.commands = None, []
    def reachable(self): pass
    def identity(self): return dict(IDENTITY)
    def shell(self, command, data=None):
        self.commands.append((command, data))
        if command.startswith('cat /data/local/tmp/start_zte_agent.sh'): return self.startup
        if command.startswith('sha256sum '): return self.installed
        if command.startswith('test -e /tmp/openui-screen/owner'): return 'active' if self.screen_active else ''
        if 'df -Pk' in command: return '1000000\n1000000\n1000000\n100'
        return ''
    def push(self, *args): raise AssertionError('a dry run attempted a write')


class Runner(unittest.TestCase):
    @classmethod
    def setUpClass(cls): cls.module = load_deploy()

    def runners(self):
        return [('run', self.module.run), ('run_threads', self.module.run_threads)]

    def test_input_output_limits_and_deadline(self):
        payload = os.urandom(4 * 1024 * 1024)
        digest = [sys.executable, '-c', 'import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())']
        for name, run in self.runners():
            with self.subTest(runner=name):
                self.assertEqual(run(digest, payload), hashlib.sha256(payload).hexdigest())
                with self.assertRaisesRegex(RuntimeError, 'large management response'):
                    run([sys.executable, '-c', 'import sys; sys.stdout.write("x" * 4000000)'])
                with self.assertRaisesRegex(RuntimeError, 'Management operation failed'):
                    run([sys.executable, '-c', 'import sys; sys.exit(3)'])
                self.assertEqual(run([sys.executable, '-c', 'import sys; sys.exit(3)'], raw=True)[0], 3)
                # Device lines through a legacy adb shell on Windows arrive as CR CR LF.
                self.assertEqual(run([sys.executable, '-c', r'import sys; sys.stdout.buffer.write(b"a\r\r\nb\r\n")']), 'a\nb')
                start = time.monotonic()
                with self.assertRaisesRegex(RuntimeError, 'timed out'):
                    run([sys.executable, '-c', 'import time; time.sleep(10)'], payload, timeout=0.3)
                self.assertLess(time.monotonic() - start, 3)

    @unittest.skipUnless(shutil.which('ssh'), 'needs an ssh client')
    def test_the_ssh_client_reports_a_refused_connection(self):
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0)); port = listener.getsockname()[1]
        transport = self.module.Transport('127.0.0.1')
        transport.command[transport.command.index('2222')] = str(port)
        # OpenSSH on Windows words it "banner exchange: Connection to UNKNOWN port -1: Connection refused".
        with self.assertRaisesRegex(RuntimeError, 'SSH to the modem failed: .*Connection refused'):
            transport.reachable()

    @unittest.skipUnless(git_bash(), 'a POSIX shell stands in for the modem')
    def test_transport_through_a_stand_in_ssh(self):
        with tempfile.TemporaryDirectory() as temp:
            ssh = Path(temp) / 'ssh.py'
            ssh.write_text(f'import subprocess, sys\nsys.exit(subprocess.call([{git_bash()!r}, "-c", sys.argv[-1]]))\n')
            transport = self.module.Transport('192.168.0.1')
            transport.command = [sys.executable, str(ssh)]
            self.assertEqual(transport.shell('printf "%s\\n" one two'), 'one\ntwo')
            with self.assertRaisesRegex(RuntimeError, 'rejected'):
                transport.shell('exit 4')
            payload = bytes(range(256)) * 4096 + b'\r\n\x1a\r'
            target = Path(temp) / 'pushed.bin'
            transport.push(payload, target.as_posix(), True)
            self.assertEqual(target.read_bytes(), payload)
            if os.name != 'nt': self.assertEqual(target.stat().st_mode & 0o777, 0o700)


class Deployment(unittest.TestCase):
    @classmethod
    def setUpClass(cls): cls.module = load_deploy()

    def args(self, **values):
        defaults = dict(agent=None, dashboard=None, harden=False, auto_adb=False, bundle=None, gateway='192.168.0.1',
                        adb_serial=None, dry_run=True, identity_file=None, keep_credentials=False, skip_unchanged=False)
        return SimpleNamespace(**{**defaults, **values})

    def test_device_scripts_lose_windows_line_endings(self):
        with tempfile.TemporaryDirectory() as temp:
            (Path(temp) / 'scripts/device').mkdir(parents=True)
            (Path(temp) / 'scripts/device/example.sh').write_bytes(b'#!/bin/sh\r\nset -eu\r\necho ok\r\n')
            with patch.object(self.module, 'ROOT', Path(temp)):
                self.assertEqual(self.module.device_script('example.sh'), b'#!/bin/sh\nset -eu\necho ok\n')
        for script in (ROOT / 'scripts/device').glob('*.sh'):
            self.assertEqual(self.module.device_script(script.name), script.read_bytes().replace(b'\r\n', b'\n'))

    def test_known_hosts_path_with_spaces_stays_one_ssh_value(self):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp) / 'John Doe'
            with patch.object(Path, 'home', return_value=home):
                command = self.module.Transport('192.168.0.1', ssh_key=str(home / '.ssh/id_ed25519')).command
        self.assertIn(f'UserKnownHostsFile="{home / ".ssh/known_hosts.d/zte"}"', command)
        self.assertEqual(command[command.index('-i') + 1], str(home / '.ssh/id_ed25519'))

    def test_ssh_failures_report_the_ssh_message(self):
        transport = self.module.Transport('192.168.0.1')
        refused = (255, '', 'Warning: something\nssh: connect to host 192.168.0.1 port 2222: Connection refused\n')
        with patch.object(self.module, 'run', return_value=refused):
            with self.assertRaisesRegex(RuntimeError, 'SSH to the modem failed: ssh: connect to host .* Connection refused'):
                transport.reachable()
        with patch.object(self.module, 'run', return_value=(0, '', 'Warning: Permanently added')):
            transport.reachable()

    def test_kept_credentials_come_from_the_installed_startup_script(self):
        password = "pa'ss word$1"
        startup = self.module.render_startup(password, '123456').decode()
        rendered = []
        original = self.module.render_startup
        def render(*values):
            rendered.append(values)
            return original(*values)
        with tempfile.TemporaryDirectory() as temp:
            agent = Path(temp) / 'zte-agent'; agent.write_bytes(aarch64())
            modem = FakeModem(startup=startup)
            with patch.object(self.module, 'Transport', return_value=modem), patch.object(self.module, 'render_startup', side_effect=render), \
                 patch.object(self.module.getpass, 'getpass', side_effect=AssertionError('prompted for a kept password')):
                self.module.deploy(self.args(agent=str(agent), keep_credentials=True))
            self.assertEqual(rendered, [(password, '123456')])
            modem = FakeModem(startup=self.module.render_startup('secret', '').decode())
            rendered.clear()
            with patch.object(self.module, 'Transport', return_value=modem), patch.object(self.module, 'render_startup', side_effect=render):
                self.module.deploy(self.args(agent=str(agent), keep_credentials=True))
            self.assertEqual(rendered, [('secret', '')])
            with patch.object(self.module, 'Transport', return_value=FakeModem(startup='#!/bin/sh\n')):
                with self.assertRaisesRegex(RuntimeError, 'No installed agent password'):
                    self.module.deploy(self.args(agent=str(agent), keep_credentials=True))

    def test_startup_check_uses_the_modem_shell_without_a_local_one(self):
        script = self.module.render_startup('secret', '')
        modem = FakeModem()
        with patch.object(self.module.shutil, 'which', return_value=None):
            self.module.check_startup(script, modem)
            self.assertEqual(modem.commands, [('sh -n', script)])
            modem.serial, modem.commands = 'serial', []
            self.module.check_startup(script, modem)
            self.assertEqual(modem.commands, [], 'adb input would be a file write; the staged check covers it')

    def test_unchanged_components_are_left_out(self):
        with tempfile.TemporaryDirectory() as temp:
            agent = Path(temp) / 'zte-agent'; agent.write_bytes(aarch64(b'agent'))
            screen = Path(temp) / 'openui-screen'; screen.write_bytes(aarch64(b'screen'))
            same = lambda path: f'{hashlib.sha256(path.read_bytes()).hexdigest()}  '
            with patch.object(self.module, 'Transport', return_value=FakeModem(screen_active=True)):
                with self.assertRaisesRegex(RuntimeError, 'stock screen'):
                    self.module.deploy(self.args(screen=str(screen)))
            modem = FakeModem(installed=same(agent) + '/data/zte-agent\n' + '0' * 64 + '  /data/bin/openui-screen\n', screen_active=True)
            args = self.args(agent=str(agent), screen=str(screen), keep_credentials=True, skip_unchanged=True)
            with patch.object(self.module, 'Transport', return_value=modem):
                with self.assertRaisesRegex(RuntimeError, 'stock screen'):
                    self.module.deploy(args)
            self.assertIsNone(args.agent, 'an unchanged agent is not replaced')
            self.assertFalse(any(c.startswith('cat /data/local/tmp/start_zte_agent.sh') for c, _ in modem.commands))
            modem = FakeModem(installed=same(agent) + '/data/zte-agent\n' + same(screen) + '/data/bin/openui-screen\n', screen_active=True)
            args = self.args(agent=str(agent), screen=str(screen), keep_credentials=True, skip_unchanged=True)
            with patch.object(self.module, 'Transport', return_value=modem), patch('builtins.print') as printed:
                self.module.deploy(args)
            self.assertIn('Nothing to deploy', ' '.join(str(c) for c in printed.call_args_list))


class DownloadRelease(unittest.TestCase):
    def test_files_are_kept_only_when_they_match_the_release_checksums(self):
        spec = importlib.util.spec_from_file_location('download_release', ROOT / 'scripts/download-release.py')
        module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        files = {'zte-agent': b'agent', 'lpac': b'lpac'}
        sums = ''.join(f'{hashlib.sha256(data).hexdigest()}  {name}\n' for name, data in files.items())
        base = 'https://github.com/owner/repo/releases/download/v2.6/'
        served = {base + 'sha256sums.txt': sums.encode(), **{base + name: data for name, data in files.items()}}
        with tempfile.TemporaryDirectory() as temp, patch.object(module, 'get', side_effect=lambda url, limit=0: served[url]), patch('builtins.print'):
            output = Path(temp)
            module.download('owner/repo', 'v2.6', output, ['zte-agent', 'lpac'])
            self.assertEqual({path.name: path.read_bytes() for path in output.iterdir()}, {**files, 'sha256sums.txt': sums.encode()})
            for path in list(output.iterdir()): path.unlink()
            served[base + 'lpac'] = b'tampered'
            with self.assertRaisesRegex(SystemExit, 'lpac does not match'):
                module.download('owner/repo', 'v2.6', output, ['zte-agent', 'lpac'])
            self.assertEqual(sorted(path.name for path in output.iterdir()), ['zte-agent'], 'no tampered file and no sha256sums.txt')
            with self.assertRaisesRegex(SystemExit, 'not listed'):
                module.download('owner/repo', 'v2.6', output, ['mihomo'])
        with self.assertRaisesRegex(SystemExit, 'HTTPS'):
            module.get('http://github.com/owner/repo')


class WindowsLauncher(unittest.TestCase):
    def test_launcher_and_script_encodings(self):
        launcher = (ROOT / 'deploy-windows.cmd').read_bytes()
        self.assertTrue(launcher.isascii(), 'cmd.exe reads batch files in the OEM code page')
        self.assertEqual(launcher.count(b'\n'), launcher.count(b'\r\n'), 'cmd.exe needs CRLF line endings')
        self.assertIn(b'scripts\\windows\\deploy.ps1', launcher)
        script = (ROOT / 'scripts/windows/deploy.ps1').read_bytes()
        self.assertTrue(script.startswith(b'\xef\xbb\xbf'), 'Windows PowerShell 5.1 reads a script without a BOM as ANSI')


if __name__ == '__main__':
    unittest.main()
