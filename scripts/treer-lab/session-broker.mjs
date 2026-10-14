// Session cookies arrive on stdin and remain in memory. The socket path is not a secret.
import http from 'node:http';
import { chmod, rm } from 'node:fs/promises';
import { stateDir } from './common.mjs';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const { cookie } = JSON.parse(input);
input = '';
const socket = `${stateDir}/session.sock`;
await rm(socket, { force: true });
const server = http.createServer((request, response) => {
  if (request.method !== 'GET' || request.url !== '/session') { response.writeHead(404).end(); return; }
  response.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  response.end(JSON.stringify({ cookie }));
});
await new Promise(resolve => server.listen(socket, resolve));
await chmod(socket, 0o600);
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => server.close(async () => {
  await rm(socket, { force: true }); process.exit(0);
}));
