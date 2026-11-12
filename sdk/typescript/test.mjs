import test from 'node:test';
import assert from 'node:assert';
import http from 'node:http';
import { ReplantClient, ReplantError } from './replant.mjs';

function server(receipt = { status: 'miss' }, status = 200) {
  const calls = [];
  const instance = http.createServer((request, response) => {
    let body = '';
    request.on('data', (chunk) => (body += chunk));
    request.on('end', () => {
      calls.push({
        method: request.method,
        url: request.url,
        auth: request.headers.authorization,
        body: body ? JSON.parse(body) : undefined,
      });
      if (request.headers.authorization === 'Bearer bad') {
        response.writeHead(401, { 'Content-Type': 'application/json' });
        response.end(JSON.stringify({ status: 'failed', reason: 'unauthorized' }));
        return;
      }
      response.writeHead(status, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify(receipt));
    });
  });
  return new Promise((resolve) => {
    instance.listen(0, '127.0.0.1', () => resolve({ instance, calls, url: `http://127.0.0.1:${instance.address().port}` }));
  });
}

test('run posts JSON with the bearer token', async () => {
  const { instance, calls, url } = await server();
  try {
    const client = new ReplantClient(url, 'secret');
    const receipt = await client.run({ prompt: 'p', input: {}, context: {} });
    assert.equal(receipt.status, 'miss');
    assert.equal(calls[0].method, 'POST');
    assert.equal(calls[0].url, '/run');
    assert.equal(calls[0].auth, 'Bearer secret');
    assert.equal(calls[0].body.prompt, 'p');
  } finally {
    instance.close();
  }
});

test('stats is GET and review/undo routes carry the right body', async () => {
  const { instance, calls, url } = await server();
  try {
    const client = new ReplantClient(url, 'secret');
    await client.stats();
    await client.approve({ prompt: 'p' }, 'a'.repeat(64));
    await client.reject({ prompt: 'p' }, 'b'.repeat(64));
    await client.undo('c'.repeat(64));
    assert.deepEqual(calls.map((call) => call.url), ['/stats', '/approve', '/reject', '/undo']);
    assert.equal(calls[0].method, 'GET');
    assert.equal(calls[1].body.candidate, 'a'.repeat(64));
    assert.equal(calls[3].body.id, 'c'.repeat(64));
  } finally {
    instance.close();
  }
});

test('HTTP errors raise ReplantError with the receipt', async () => {
  const { instance, url } = await server();
  try {
    const client = new ReplantClient(url, 'bad');
    await assert.rejects(
      client.run({ prompt: 'p' }),
      (error) => error instanceof ReplantError && error.status === 401 && error.receipt.status === 'failed',
    );
  } finally {
    instance.close();
  }
});

test('inspect posts to the read-only endpoint', async () => {
  const { instance, calls, url } = await server();
  try {
    const request = { prompt: 'p' };
    await new ReplantClient(url, 'secret').inspect(request);
    assert.equal(calls[0].url, '/inspect');
    assert.equal(calls[0].method, 'POST');
    assert.deepEqual(calls[0].body, request);
  } finally {
    instance.close();
  }
});

test('rerecord explicitly posts only the selected record id', async () => {
  const { instance, calls, url } = await server({ status: 'succeeded', operation: 'rerecord' });
  try {
    const receipt = await new ReplantClient(url, 'secret').rerecord('d'.repeat(64));
    assert.equal(receipt.operation, 'rerecord');
    assert.deepEqual(calls, [{ method: 'POST', url: '/rerecord', auth: 'Bearer secret', body: { id: 'd'.repeat(64) } }]);
  } finally {
    instance.close();
  }
});

test('stale responses retain the template and never automatically re-record', async () => {
  const expected = { status: 'stale', stale_record: { id: 'd'.repeat(64), recipe: { steps: [] } } };
  const { instance, calls, url } = await server(expected);
  try {
    assert.deepEqual(await new ReplantClient(url, 'secret').run({ prompt: 'p' }), expected);
    assert.deepEqual(calls.map((call) => call.url), ['/run']);
  } finally {
    instance.close();
  }
});

test('failed tool stdout, stderr and exit code survive the HTTP error', async () => {
  const expected = { status: 'failed', calls: [{ exit_code: 1, output: { ok: false }, stdout: 'seed failed', stderr: 'tail' }] };
  const { instance, calls, url } = await server(expected, 422);
  try {
    await assert.rejects(new ReplantClient(url, 'secret').record({}), (error) => {
      assert.equal(error.status, 422);
      assert.deepEqual(error.receipt, expected);
      return true;
    });
    assert.equal(calls.length, 1);
  } finally {
    instance.close();
  }
});
