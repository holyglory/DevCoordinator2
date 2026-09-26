import { i18n } from './i18n.mjs';
try {
  await i18n.ready;
  // Fetch independent scripts together but execute in their existing global order.
  await Promise.all(['glossary.js','artifact-content.js','artifacts.js','tests.js','workspace.js','performance.js','health.js','app.js'].map(file => new Promise((resolve,reject) => {
    const script = document.createElement('script'); script.src = `/${file}`; script.async = false;
    script.onload = resolve; script.onerror = () => reject(new Error('Console script unavailable'));
    document.body.appendChild(script);
  })));
} catch {
  const main = document.getElementById('main');
  main.replaceChildren();
  const message = document.createElement('p'); message.textContent = 'The Console could not load. Reload to try again.';
  const retry = document.createElement('button'); retry.className = 'btn'; retry.textContent = 'Reload'; retry.addEventListener('click', () => location.reload());
  main.append(message,retry);
}
