"""Run with python3 -m unittest discover -s deploy -p 'test_*.py'."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name('install.sh').resolve()


class DirectorySetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.config = self.root / 'etc' / 'tapo-telemetry'
        self.state = self.root / 'var' / 'lib' / 'tapo-telemetry'
        self.spool = self.state / 'spool'
        mocks = self.root / 'commands'
        mocks.mkdir()
        # Account administration is simulated; mkdir/chmod/find use real files.
        for name, body in {
            'id': 'case "$*" in "-u") echo 0;; "-u tapoagent") echo 123;; "-g tapoagent") echo 456;; esac',
            'getent': 'echo tapoagent:x:456:',
            'chown': 'exit 0',
            'useradd': 'exit 99',
        }.items():
            path = mocks / name
            path.write_text('#!/bin/bash\n' + body + '\n')
            path.chmod(0o755)
        self.env = dict(os.environ, PATH=str(mocks) + ':' + os.environ['PATH'])
        self.mocks = mocks

    def run_setup(self):
        if not SCRIPT.exists():
            self.fail('Directory setup script has not been implemented')
        return subprocess.run(
            ['bash', '-c', 'source "$1"; config_dir="$2"; state_dir="$3"; spool_dir="$3/spool"; prepare_directories',
             'test', str(SCRIPT), str(self.config), str(self.state)],
            env=self.env, capture_output=True, text=True,
        )

    def test_creates_private_directories(self):
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        for directory in [self.config, self.state, self.spool]:
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)

    def test_repeat_fixes_permissions_without_changing_contents(self):
        self.config.mkdir(parents=True)
        self.config.chmod(0o755)
        self.spool.mkdir(parents=True)
        files = [self.config / 'credentials.json', self.spool / 'pending.json', self.spool / '.lock']
        for file in files:
            file.write_text('keep this content')
            file.chmod(0o644)
        for _ in range(2):
            result = self.run_setup()
            self.assertEqual(result.returncode, 0, result.stderr)
            for file in files:
                self.assertEqual(file.read_text(), 'keep this content')
                self.assertEqual(file.stat().st_mode & 0o777, 0o600)

    def test_rejects_symlink_before_changing_permissions(self):
        self.config.mkdir(parents=True)
        self.spool.mkdir(parents=True)
        victim = self.root / 'victim'
        victim.write_text('preserve')
        victim.chmod(0o644)
        (self.spool / 'pending.json').symlink_to(victim)
        original_mode = self.config.stat().st_mode & 0o777
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unsafe', result.stderr.lower())
        self.assertEqual(victim.stat().st_mode & 0o777, 0o644)
        self.assertEqual(self.config.stat().st_mode & 0o777, original_mode)

    def test_rejects_privileged_service_account(self):
        (self.mocks / 'id').write_text('#!/bin/bash\necho 0\n')
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unprivileged', result.stderr)
        self.assertFalse(self.config.exists())

    def test_creates_missing_service_account(self):
        marker = self.root / 'account-created'
        (self.mocks / 'id').write_text(
            '#!/bin/bash\ncase "$*" in "-u") echo 0;; '
            f'"-u tapoagent") [[ -f {marker} ]] || exit 1; echo 123;; '
            '"-g tapoagent") echo 456;; esac\n'
        )
        (self.mocks / 'useradd').write_text(f'#!/bin/bash\ntouch {marker}\n')
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(marker.exists())
        self.assertEqual(self.spool.stat().st_mode & 0o777, 0o700)

    def test_rejects_symlink_directory_and_non_directory_destination(self):
        for symlink in [False, True]:
            with self.subTest(symlink=symlink):
                self.config.parent.mkdir(parents=True, exist_ok=True)
                if symlink:
                    self.config.symlink_to(self.root, target_is_directory=True)
                else:
                    self.config.write_text('preserve')
                try:
                    result = self.run_setup()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(self.state.exists())
                finally:
                    self.config.unlink()


if __name__ == '__main__':
    unittest.main()
