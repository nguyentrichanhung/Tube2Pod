import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const script = await readFile(new URL('../assets/connection.js', import.meta.url), 'utf8');

test('repeated generation while pending issues one ticket and displays it', async () => {
  let click, resolveTicket, requests = 0, encoded;
  const button = { disabled: false, addEventListener: (_, handler) => { click = handler; } };
  const status = { textContent: '' };
  const qr = { innerHTML: '', textContent: '' };
  const envelope = { ticket: 'valid-ticket', expires_at: Math.floor(Date.now() / 1000) + 300 };
  const elements = { '#generate': button, '#pair-status': status, '#qr': qr, 'meta[name="csrf-token"]': { content: 'token' } };
  const context = {
    document: { querySelector: key => elements[key], querySelectorAll: () => [] },
    fetch: path => {
      if (path.endsWith('/status')) return Promise.resolve({ ok: true, json: async () => ({ status: 'pending' }) });
      requests++;
      return new Promise(resolve => { resolveTicket = resolve; });
    },
    qrcode: () => ({ addData: data => { encoded = data; }, make() {}, createImgTag: () => '<img>' }),
    URLSearchParams, Date, setInterval: () => 1, clearInterval() {},
  };
  vm.runInNewContext(script, context);
  const first = click();
  assert.equal(button.disabled, true);
  await click();
  assert.equal(requests, 1);
  resolveTicket({ ok: true, json: async () => envelope });
  await first;
  assert.equal(button.disabled, false);
  assert.deepEqual(JSON.parse(encoded), envelope);
  assert.equal(qr.innerHTML, '<img>');
});
