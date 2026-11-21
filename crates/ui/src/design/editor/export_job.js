const [url, screens] = await dioxus.recv();
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));

async function request(path, options) {
  return fetch(path, { ...options, cache: 'no-store', signal: AbortSignal.timeout(15000) });
}

async function readStatus(response) {
  if (!response.ok) {
    const body = await response.json().catch(() => null);
    throw new Error(body?.error?.message || `Cannot prepare PNG export: HTTP ${response.status}.`);
  }
  return response.json();
}

try {
  let status = await readStatus(await request(url, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ screens }),
  }));
  const statusUrl = `${url}/${encodeURIComponent(status.job_id)}`;
  let previous = -1;
  let failures = 0;
  while (status.state === 'preparing') {
    if (previous !== status.completed) {
      dioxus.send({ state: 'progress', completed: status.completed, total: status.total });
      previous = status.completed;
    }
    await wait(1500);
    let response;
    try {
      response = await request(statusUrl);
      failures = 0;
    } catch (error) {
      if (++failures < 5) continue;
      throw new Error('Lost the connection while checking PNG export progress. Check your connection and try again.');
    }
    status = await readStatus(response);
  }
  if (status.state !== 'ready') {
    throw new Error('Could not prepare the PNG ZIP. Try again or export fewer screens.');
  }
  const href = `${statusUrl}/download`;
  const link = document.createElement('a');
  link.href = href;
  link.download = '';
  document.body.appendChild(link);
  link.click();
  link.remove();
  dioxus.send({ state: 'ready', href });
} catch (error) {
  dioxus.send({ state: 'failed', message: error.message || 'Could not prepare the PNG ZIP. Try again.' });
}
