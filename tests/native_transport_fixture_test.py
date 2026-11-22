import base64
import json
import socket
import ssl
import subprocess

fixture = subprocess.Popen(['python3', 'tests/native_transport_fixture.py'], stdout=subprocess.PIPE, text=True)
try:
    info = json.loads(fixture.stdout.readline())
    def get(port, expected, path='/', credential='fixture-secret'):
        # The test client performs the same exact-key check before sending HTTP.
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE
        with context.wrap_socket(socket.create_connection(('127.0.0.1', port)), server_hostname='fixture') as sock:
            cert = sock.getpeercert(binary_form=True)
            pem = subprocess.check_output(['openssl', 'x509', '-inform', 'DER', '-pubkey', '-noout'], input=cert)
            key = base64.b64encode(subprocess.check_output(['openssl', 'pkey', '-pubin', '-outform', 'DER'], input=pem)).decode()
            if key != expected:
                return None
            sock.sendall(f'GET {path} HTTP/1.0\r\nAuthorization: {credential}\r\n\r\n'.encode())
            response = b''
            while chunk := sock.recv(65536):
                response += chunk
            return response.split(b'\r\n\r\n', 1)[1]
    assert get(info['port'], info['pin']) == b'fixture api'
    assert get(info['relay'], info['pin']) == b'fixture api'
    assert get(info['relay'], 'wrong', credential='must-not-arrive') is None
    received = json.loads(get(info['port'], info['pin'], '/received'))
    assert 'must-not-arrive' not in received
    print('direct and outbound relay TLS, wrong-key refusal before HTTP: passed')
finally:
    fixture.terminate()
    fixture.wait()
