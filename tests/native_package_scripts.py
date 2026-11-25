"""Check native package payload and application entry point."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

repo = Path(__file__).resolve().parent.parent
if sys.platform.startswith('linux'):
    with tempfile.TemporaryDirectory(prefix='native package check ') as tmp:
        root = Path(tmp)
        binary = root / 'binary with spaces'
        binary.write_bytes(Path('/bin/true').read_bytes())
        out = root / 'output with spaces'
        script = repo / 'deploy/installers/package.sh'
        assert subprocess.run([str(script), 'unsupported', str(binary), str(out)], capture_output=True).returncode != 0
        subprocess.run([str(script), 'x86_64-unknown-linux-gnu', str(binary), str(out)], check=True)
        package = out / 'tube2pod-x86_64-unknown-linux-gnu.deb'
        assert subprocess.check_output(['dpkg-deb', '-f', str(package), 'Architecture'], text=True).strip() == 'amd64'
        payload = root / 'payload'
        subprocess.run(['dpkg-deb', '-x', str(package), str(payload)], check=True)
        files = {str(p.relative_to(payload)) for p in payload.rglob('*') if p.is_file()}
        assert files == {'usr/bin/tube2pod', 'usr/share/applications/tube2pod.desktop'}, files
        assert (payload / 'usr/bin/tube2pod').read_bytes() == binary.read_bytes()
        assert os.access(payload / 'usr/bin/tube2pod', os.X_OK)
        for directory in (payload / 'usr', payload / 'usr/bin', payload / 'usr/share', payload / 'usr/share/applications'):
            assert directory.stat().st_mode & 0o777 == 0o755
        assert (payload / 'usr/share/applications/tube2pod.desktop').stat().st_mode & 0o777 == 0o644
        assert 'Mở Tube2Pod' in subprocess.check_output(['dpkg-deb', '-f', str(package), 'Description'], text=True)
        assert 'Exec=/usr/bin/tube2pod --desktop\n' in (payload / 'usr/share/applications/tube2pod.desktop').read_text()
print('Native package script checks passed')
