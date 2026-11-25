"""Exercise archive builders with paths containing spaces and missing input."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

repo = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='package self check ') as tmp:
    root = Path(tmp)
    binary = root / 'fake binary'
    binary.write_bytes(b'package fixture only')
    out = root / 'output archives'
    if os.name == 'nt':
        command = ['powershell', '-NoProfile', '-NonInteractive', '-File', str(repo / 'deploy/package.ps1')]
        targets = ['x86_64-pc-windows-msvc']
    else:
        command = [str(repo / 'deploy/package.sh')]
        targets = ['x86_64-unknown-linux-gnu', 'aarch64-apple-darwin', 'x86_64-apple-darwin']
    assert subprocess.run(command, capture_output=True).returncode != 0
    for target in targets:
        def invocation(source):
            return command + (['-Target', target, '-Binary', str(source), '-OutDir', str(out)] if os.name == 'nt'
                              else [target, str(source), str(out)])
        assert subprocess.run(invocation(root / 'missing'), capture_output=True).returncode != 0
        subprocess.run(invocation(binary), check=True)
        archive = out / f'tube2pod-{target}.{ "zip" if os.name == "nt" else "tar.gz" }'
        subprocess.run([sys.executable, str(repo / 'tests/package_smoke.py'), str(archive),
                        '--archive-only', '--work-dir', str(root / target)], check=True)
print('Package script self-check passed')
