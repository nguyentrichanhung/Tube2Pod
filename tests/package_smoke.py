"""Standard-library package checks and native launcher smoke, always isolated data."""
import argparse
import html
import http.cookiejar
import json
import os
from pathlib import Path
import re
import signal
import socket
import sqlite3
import subprocess
import tarfile
import tempfile
import time
import urllib.parse
import urllib.request
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument('archive', type=Path)
parser.add_argument('--work-dir', type=Path)
parser.add_argument('--archive-only', action='store_true')
parser.add_argument('--url', help='Optional authorized real YouTube source; requires Internet')
args = parser.parse_args()
root = args.work_dir or Path(tempfile.mkdtemp(prefix='tube2pod package smoke '))
root.mkdir(parents=True, exist_ok=True)
assert not (root / 'persistent data').exists(), 'Use a fresh smoke directory'
name = args.archive.name.removesuffix('.tar.gz').removesuffix('.zip')
windows = name.endswith('windows-msvc')
expected = {'tube2pod.exe' if windows else 'tube2pod', 'start.ps1' if windows else 'start.sh',
            'portable.env.example', 'Caddyfile.example', 'portable.md', 'direct-https.md'}
if name.endswith('linux-gnu'):
    expected |= {'deploy/install-linux-x86_64.sh', 'deploy/tube2pod.service', 'deploy/tube2pod.env.example'}
if windows:
    with zipfile.ZipFile(args.archive) as archive:
        files = {p.replace('\\', '/').removeprefix(name + '/') for p in archive.namelist() if not p.endswith('/')}
        assert files == expected, files
        archive.extractall(root)
else:
    with tarfile.open(args.archive) as archive:
        files = {p.name.removeprefix(name + '/') for p in archive if p.isfile()}
        assert files == expected, files
        assert all(p.isdir() or p.isfile() for p in archive), 'unexpected archive entry'
        archive.extractall(root)
package = root / name
if not windows:
    assert os.access(package / 'start.sh', os.X_OK)
    assert os.access(package / 'tube2pod', os.X_OK)

if args.archive_only:
    print('Archive contents and permissions passed:', args.archive)
    raise SystemExit(0)

# Independent sockets, not ports from a user's config.
def port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]
ports = {key: port() for key in ('ADMIN', 'PUBLIC', 'OWNER', 'LAN')}
config = root / 'settings with spaces.env'
data = root / 'persistent data'
values = {'DATA_DIR': str(data), 'PUBLIC_BASE_URL': 'https://localhost:19844'}
for key in ('YTDLP_PATH', 'FFMPEG_PATH', 'FFPROBE_PATH'):
    if os.environ.get(key):
        values[key] = os.environ[key]
values.update({f'BIND_{key}': f'127.0.0.1:{value}' for key, value in ports.items()})
config.write_text(''.join(f'{k}={v}\n' for k, v in values.items()), encoding='utf-8')
env = {k: v for k, v in os.environ.items() if k not in values and not k.startswith('RELAY_')}
base = f'http://localhost:{ports["OWNER"]}'
cookies = http.cookiejar.CookieJar()
client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cookies))

def get(path):
    with client.open(base + path, timeout=15) as response:
        return response.read().decode()

def post(path, fields):
    csrf = next(c.value for c in cookies if c.name == 't2p_csrf')
    fields = {'_csrf': csrf, **fields}
    # urllib lacks browsers' secure-cookie exception for localhost.
    headers = {'Origin': base, 'Cookie': f't2p_csrf={csrf}'}
    if path == '/api/preview':
        headers['HX-Request'] = 'true'
    request = urllib.request.Request(base + path, urllib.parse.urlencode(fields).encode(), headers=headers)
    with client.open(request, timeout=90) as response:
        return response.read().decode()

def records():
    with sqlite3.connect(data / 'tube2pod.db') as db:
        show = db.execute('SELECT title,feed_token FROM shows ORDER BY id').fetchall()
        episodes = db.execute('SELECT uuid,status,audio_path,audio_bytes FROM episodes ORDER BY uuid').fetchall()
        return show, episodes

log = (root / 'server.log').open('wb')
command = (['powershell', '-NoProfile', '-File', str(package / 'start.ps1'), '-Config', str(config)] if windows
           else [str(package / 'start.sh'), str(config)])
process = None

def start():
    global process
    process = subprocess.Popen(command, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT,
                               start_new_session=not windows)
    for _ in range(150):
        if process.poll() is not None:
            raise AssertionError((root / 'server.log').read_text(errors='replace'))
        try:
            html = get('/shows')
            assert 'Tube2Pod' in html
            return
        except (OSError, urllib.error.URLError):
            time.sleep(.2)
    raise AssertionError('owner page did not become ready')

def stop():
    if process and process.poll() is None:
        if windows:
            subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], check=True, capture_output=True)
        else:
            os.killpg(process.pid, signal.SIGINT)
        process.wait(timeout=15)

try:
    start()
    post('/settings', {'owner_name': 'Package smoke', 'owner_email': 'smoke@example.com',
                      'min_source_seconds': '10', 'max_source_minutes': '180'})
    post('/shows', {'title': 'Package smoke', 'description': 'Isolated native package test',
                   'category': 'Technology', 'language': 'en'})
    if args.url:
        preview = post('/api/preview', {'url': args.url, 'show_id': '1'})
        (root / 'preview.html').write_text(preview)
        track = re.search(r'name="source_audio_track"[^>]*value="([^"]+)"', preview)
        assert track, 'No real audio track in preview; inspect preview.html/server.log'
        video = urllib.parse.parse_qs(urllib.parse.urlparse(args.url).query)['v'][0]
        post('/episodes', {'url': args.url, 'show_id': '1', 'preview_video': video,
                           'source_audio_track': html.unescape(track[1]), 'language': 'en'})
        for _ in range(180):
            rows = records()[1]
            if rows and rows[0][1] in ('ready', 'failed'):
                break
            time.sleep(1)
        assert rows and rows[0][1] == 'ready', rows
        post('/shows/1/feed/toggle', {'enabled': 'on'})
        post('/shows/1/publish-ready', {'confirm_title': 'Package smoke'})
        assert records()[1][0][1] == 'published'
    before = records()
    audio = {p: Path(p).read_bytes() for _, _, p, _ in before[1] if p}
    stop()
    start()
    assert records() == before, 'show/feed token/episode changed after restart'
    assert all(Path(p).read_bytes() == content for p, content in audio.items())
    result = {'archive': str(args.archive.resolve()), 'work_dir': str(root), 'ports': ports,
              'shows_preserved': len(before[0]), 'audio_preserved': len(audio), 'real_source': args.url,
              'passed': True, 'windows_stop': 'taskkill tree (not manual Ctrl+C)' if windows else None}
    (root / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
finally:
    stop()
    log.close()
