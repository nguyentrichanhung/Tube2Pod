"""Install a native package on a disposable runner and exercise its desktop launcher."""
import json
import os
from pathlib import Path
import shlex
import signal
import sqlite3
import socket
import subprocess
import struct
import sys
import tempfile
import time
import urllib.request

package = Path(sys.argv[1]).resolve()
root = Path(tempfile.mkdtemp(prefix='native installer smoke '))
data = root / 'persistent data'
env = os.environ.copy()
for key in list(env):
    if key in ('YTDLP_PATH', 'FFMPEG_PATH', 'FFPROBE_PATH') or key.startswith(('RELAY_', 'TUBE2POD_RELAY_')):
        env.pop(key)
env['DATA_DIR'] = str(data)
for key in ('ADMIN', 'PUBLIC', 'OWNER', 'LAN'):
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        env['BIND_' + key] = '127.0.0.1:' + str(sock.getsockname()[1])
env['PUBLIC_BASE_URL'] = 'https://localhost:19844'
windows = sys.platform == 'win32'
mac = sys.platform == 'darwin'
if windows:
    install = [str(package), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART']
    app = Path(os.environ['LOCALAPPDATA']) / 'Programs' / 'Tube2Pod'
    uninstall = [str(app / 'unins000.exe'), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART']
elif mac:
    install = ['sudo', 'installer', '-pkg', str(package), '-target', '/']
    app = Path('/Applications/Tube2Pod.app')
    uninstall = ['sudo', 'rm', '-rf', str(app)]
else:
    install = ['sudo', 'dpkg', '-i', str(package)]
    uninstall = ['sudo', 'dpkg', '-r', 'tube2pod']

def check_architecture(binary):
    with open(binary, 'rb') as source:
        header = source.read(4096)
    if windows:
        offset = struct.unpack_from('<I', header, 0x3c)[0]
        assert header[offset:offset + 4] == b'PE\0\0'
        assert struct.unpack_from('<H', header, offset + 4)[0] == 0x8664
    elif mac:
        assert header[:4] == b'\xcf\xfa\xed\xfe'
        cpu = struct.unpack_from('<I', header, 4)[0]
        assert cpu == (0x0100000c if 'aarch64' in package.name else 0x01000007)
        libraries = subprocess.check_output(['otool', '-L', str(binary)], text=True).splitlines()[1:]
        for line in libraries:
            library = line.strip().split(' (compatibility version', 1)[0]
            assert library.startswith(('/usr/lib/', '/System/Library/')), 'Non-system macOS dependency: ' + library
    else:
        assert header[:4] == b'\x7fELF'
        assert struct.unpack_from('<H', header, 18)[0] == 62

def launcher():
    if windows:
        shortcut = Path(os.environ['APPDATA']) / 'Microsoft/Windows/Start Menu/Programs/Tube2Pod/Tube2Pod.lnk'
        script = "$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:TUBE2POD_SHORTCUT); @($s.TargetPath,$s.Arguments) | ConvertTo-Json -Compress"
        shortcut_env = {**os.environ, 'TUBE2POD_SHORTCUT': str(shortcut)}
        target, arguments = json.loads(subprocess.check_output(['powershell', '-NoProfile', '-Command', script], text=True, env=shortcut_env))
        assert arguments == '--desktop'
        return [target, arguments]
    if mac:
        return [str(app / 'Contents/MacOS/launch')]
    entry = Path('/usr/share/applications/tube2pod.desktop').read_text()
    return shlex.split(next(line[5:] for line in entry.splitlines() if line.startswith('Exec=')))

def wait_for(fn, seconds=300):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = fn()
            if value:
                return value
        except (OSError, ValueError):
            pass
        time.sleep(.5)
    raise AssertionError('Timed out waiting for native launcher')

def session():
    return json.loads((data / 'desktop-session.json').read_text())

def request(info, path, action=None):
    headers = {'x-desktop-token': info['token']}
    if action:
        headers.update({'Origin': info['url'], 'x-desktop-action': action})
    req = urllib.request.Request(info['url'] + path, data=b'' if action else None, headers=headers)
    with urllib.request.urlopen(req, timeout=10) as response:
        return response.read()

def state(info):
    return json.loads(request(info, '/status'))['status']

def expect(info, desired, seconds=300):
    def check():
        current = state(info)
        if current in ('error', 'full'):
            raise AssertionError('Desktop setup failed: ' + current)
        return current == desired
    wait_for(check, seconds)

process = None
owned_pid = None

def mac_pids():
    lines = subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True).splitlines()
    command = str(app / 'Contents/MacOS/tube2pod') + ' --desktop'
    return {int(parts[0]) for line in lines if len(parts := line.strip().split(None, 1)) == 2 and parts[1] == command}

def mac_open():
    command = ['open', '-a', str(app)]
    for key in ('DATA_DIR', 'PUBLIC_BASE_URL', 'BIND_ADMIN', 'BIND_PUBLIC', 'BIND_OWNER', 'BIND_LAN'):
        command += ['--env', key + '=' + env[key]]
    for key in ('YTDLP_PATH', 'FFMPEG_PATH', 'FFPROBE_PATH'):
        command += ['--env', key + '=']
    subprocess.run(command, check=True, env=env)

def launch():
    global process, owned_pid
    if mac:
        previous = mac_pids()
        mac_open()
        owned_pid = wait_for(lambda: next(iter(mac_pids() - previous), None))
    else:
        process = subprocess.Popen(launcher(), env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def shutdown():
    global process, owned_pid
    if mac and owned_pid:
        os.kill(owned_pid, signal.SIGTERM)
        wait_for(lambda: owned_pid not in mac_pids())
        owned_pid = None
    elif process:
        process.terminate()
        process.wait(timeout=20)
        process = None

try:
    subprocess.run(install, check=True)
    check_architecture((app / 'tube2pod.exe') if windows else (app / 'Contents/MacOS/tube2pod') if mac else Path('/usr/bin/tube2pod'))
    launch()
    info = wait_for(session)
    expect(info, 'prepare')
    request(info, '/action', 'install')
    expect(info, 'ready', 900)
    suffix = '.exe' if windows else ''
    for name in ('yt-dlp', 'ffmpeg', 'ffprobe'):
        tool = data / 'tools' / (name + suffix)
        assert tool.is_file(), 'Setup did not install ' + name + ' in isolated data'
        subprocess.run([str(tool), '--version' if name == 'yt-dlp' else '-version'], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    request(info, '/action', 'start')
    expect(info, 'running')
    if mac:
        mac_open()
        assert state(info) == 'running'
    # Seed a deterministic published episode; no network source is used for this fixture.
    audio = data / 'audio' / '00000000-0000-4000-8000-000000000001-1.mp3'
    audio.write_bytes((Path(__file__).parent / 'fixtures/audio_sine.mp3').read_bytes())
    cover = data / 'shows' / 'fixture-cover.jpg'
    cover.write_bytes(b'fixture cover')
    token = 'installer-smoke-feed-token'
    with sqlite3.connect(data / 'tube2pod.db') as db:
        db.execute("INSERT INTO shows (id,title,description,category,owner_name,owner_email,cover_path,feed_token,feed_enabled,created_at,updated_at) VALUES (1,'Installer fixture','Fixture show','Technology','Fixture','fixture@example.com',?,?,1,1,1)", (str(cover), token))
        db.execute("INSERT INTO episodes (uuid,show_id,video_id,source_url,source_title,language,stage,status,audio_path,audio_bytes,audio_duration_s,published_at,created_at,updated_at) VALUES ('00000000-0000-4000-8000-000000000001',1,'fixture','https://example.com/fixture','Fixture episode','en','complete','published',?,?,1,1,1,1)", (str(audio), audio.stat().st_size))
    feed_path = '/feed/' + token + '.xml'
    audio_path = '/audio/' + token + '/00000000-0000-4000-8000-000000000001-1.mp3'
    public_origin = 'http://' + env['BIND_PUBLIC']
    def public_get(path):
        try:
            with urllib.request.urlopen(public_origin + path, timeout=10) as response:
                return response.read()
        except OSError:
            raise AssertionError('Failed to fetch fixture feed or audio') from None
    before_feed = public_get(feed_path)
    before_audio = public_get(audio_path)
    assert b'Fixture episode' in before_feed
    assert ('https://localhost:19844' + audio_path).encode() in before_feed
    assert before_audio == audio.read_bytes()
    identity = (data / 'identity/server-id').read_bytes()
    identity_key = (data / 'identity/tls-key.pem').read_bytes()
    request(info, '/action', 'stop')
    expect(info, 'ready')
    shutdown()
    subprocess.run(uninstall, check=True)
    assert audio.read_bytes() == before_audio
    subprocess.run(install, check=True)
    assert audio.read_bytes() == before_audio
    launch()
    info = wait_for(lambda: session() if state(session()) == 'running' else None)
    assert (data / 'identity/server-id').read_bytes() == identity
    assert (data / 'identity/tls-key.pem').read_bytes() == identity_key
    assert public_get(feed_path) == before_feed
    assert public_get(audio_path) == before_audio
    request(info, '/action', 'stop')
    expect(info, 'ready')
    print('Native install, tool setup, start, stop, uninstall and reinstall passed; published fixture RSS and MP3 preserved (no real source)')
finally:
    shutdown()
    subprocess.run(uninstall, check=False)
