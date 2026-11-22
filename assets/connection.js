(() => {
  const csrf = document.querySelector('meta[name="csrf-token"]').content;
  const status = document.querySelector('#pair-status');
  let timer;
  let activeTicket;
  async function post(path, data = {}) {
    const body = new URLSearchParams({ _csrf: csrf, ...data });
    const response = await fetch(path, { method: 'POST', body });
    if (!response.ok) throw new Error('Không thể thực hiện. Hãy thử lại.');
    return response.json();
  }
  document.querySelector('#generate').addEventListener('click', async () => {
    try {
      const envelope = await post('/connection/pair-ticket');
      const qr = qrcode(0, 'M');
      qr.addData(JSON.stringify(envelope));
      qr.make();
      document.querySelector('#qr').innerHTML = qr.createImgTag(3, 12);
      activeTicket = envelope.ticket;
      const update = async () => {
        const remaining = Math.max(0, Math.ceil(envelope.expires_at - Date.now() / 1000));
        status.textContent = remaining ? `Còn ${Math.floor(remaining / 60)}:${String(remaining % 60).padStart(2, '0')}` : 'Mã đã hết hạn. Tạo mã mới.';
        if (!remaining) { clearInterval(timer); activeTicket = null; document.querySelector('#qr').textContent = 'Mã đã hết hạn'; return; }
        try {
          const result = await post('/connection/pair-ticket/status', { ticket: envelope.ticket });
          if (activeTicket !== envelope.ticket) return;
          if (result.status !== 'pending') {
            clearInterval(timer); activeTicket = null;
            const message = result.status === 'used' ? 'Mã đã được dùng. Tạo mã mới.' : 'Mã không còn hiệu lực. Tạo mã mới.';
            status.textContent = message; document.querySelector('#qr').textContent = message;
          }
        } catch (_) { /* Keep the countdown while the status is unavailable. */ }
      };
      clearInterval(timer); update(); timer = setInterval(update, 1000);
    } catch (error) { status.textContent = error.message; }
  });
  document.querySelectorAll('.device-row').forEach(row => {
    row.querySelector('.rename')?.addEventListener('submit', async event => {
      event.preventDefault();
      try { await post(`/connection/devices/${encodeURIComponent(row.dataset.id)}/rename`, { label: event.target.label.value }); location.reload(); }
      catch (error) { status.textContent = error.message; }
    });
    row.querySelector('.revoke')?.addEventListener('click', async () => {
      if (!confirm(`Thu hồi quyền quản trị của ${row.querySelector('strong').textContent}?`)) return;
      try { await post(`/connection/devices/${encodeURIComponent(row.dataset.id)}/revoke`); location.reload(); }
      catch (error) { status.textContent = error.message; }
    });
  });
})();
