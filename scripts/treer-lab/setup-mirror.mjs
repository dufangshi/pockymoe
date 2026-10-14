// Serves only the unchanged setup bundle and pinned release assets on the lab network.
import http from 'node:http';
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
const allowed = new Set(['treer-setup.sh', 'v0.13.2/SHA256SUMS',
  'v0.13.2/remote-codex-linux-x64-gnu', 'v0.13.2/remote-codex-web.zip', 'v0.13.2/runtime-version.txt']);
http.createServer(async (request, response) => {
  const file = request.url?.slice(1);
  if (!allowed.has(file) || !['GET', 'HEAD'].includes(request.method)) {
    response.writeHead(404).end(); return;
  }
  try {
    const target = `/mirror/${file}`;
    response.writeHead(200, { 'content-length': (await stat(target)).size,
      'content-type': 'application/octet-stream' });
    if (request.method === 'HEAD') response.end();
    else createReadStream(target).pipe(response);
  } catch { response.writeHead(404).end(); }
}).listen(19000, '0.0.0.0');
