(() => {
  const media = matchMedia('(prefers-color-scheme: dark)');
  let choice;
  try { choice = localStorage.getItem('theme'); } catch (_) {}
  if (!['light', 'dark', 'system'].includes(choice)) choice = 'system';
  function apply() {
    document.documentElement.dataset.theme = choice === 'system' ? (media.matches ? 'dark' : 'light') : choice;
    document.querySelectorAll('input[name="theme"]').forEach(input => { input.checked = input.value === choice; });
  }
  apply();
  media.addEventListener('change', apply);
  document.addEventListener('change', event => {
    if (event.target.name !== 'theme') return;
    choice = event.target.value;
    try { localStorage.setItem('theme', choice); } catch (_) {}
    apply();
  });
  document.addEventListener('DOMContentLoaded', apply);
})();
