'use strict';

const test = require('node:test');
const assert = require('node:assert');
const { Replant } = require('../nodes/Replant/Replant.node.js');
const { ReplantApi } = require('../credentials/ReplantApi.credentials.js');

function context(operation, payload, response = { status: 'miss' }) {
  const calls = [];
  return {
    calls,
    getInputData: () => [{ json: {} }],
    getCredentials: async () => ({ baseUrl: 'http://replant:8799/', token: 'secret' }),
    getNodeParameter: (name) => ({ operation, payload })[name],
    helpers: {
      httpRequest: async (options) => {
        calls.push(options);
        return response;
      },
    },
  };
}

test('run posts the parsed payload to /run with the bearer token', async () => {
  const ctx = context('run', '{"prompt":"p","input":{},"context":{}}');
  const output = await new Replant().execute.call(ctx);
  assert.equal(output[0][0].json.status, 'miss');
  assert.equal(ctx.calls[0].method, 'POST');
  assert.equal(ctx.calls[0].url, 'http://replant:8799/run');
  assert.equal(ctx.calls[0].headers.Authorization, 'Bearer secret');
  assert.deepEqual(ctx.calls[0].body, { prompt: 'p', input: {}, context: {} });
});

test('record, approve and undo route to their endpoints', async () => {
  for (const operation of ['record', 'approve', 'undo']) {
    const ctx = context(operation, '{"id":"abc"}', { status: 'succeeded' });
    await new Replant().execute.call(ctx);
    assert.equal(ctx.calls[0].url, `http://replant:8799/${operation}`);
    assert.equal(ctx.calls[0].method, 'POST');
  }
});

test('stats uses GET without a body', async () => {
  const ctx = context('stats', '{}', { executions: 1 });
  const output = await new Replant().execute.call(ctx);
  assert.equal(ctx.calls[0].method, 'GET');
  assert.equal(ctx.calls[0].url, 'http://replant:8799/stats');
  assert.equal(ctx.calls[0].body, undefined);
  assert.equal(output[0][0].json.executions, 1);
});

test('invalid JSON and missing base URL are explicit errors', async () => {
  await assert.rejects(
    new Replant().execute.call(context('run', '{broken')),
    /not valid JSON/,
  );
  const ctx = context('run', '{}');
  ctx.getCredentials = async () => ({ baseUrl: '', token: 'secret' });
  await assert.rejects(new Replant().execute.call(ctx), /Base URL is required/);
});

test('credential declares the header and health test', () => {
  const credential = new ReplantApi();
  assert.equal(credential.name, 'replantApi');
  assert.equal(credential.authenticate.properties.headers.Authorization, '=Bearer {{$credentials.token}}');
  assert.equal(credential.test.request.url, '/healthz');
});

test('inspect uses /inspect and wraps missing records as valid n8n JSON', async () => {
  const ctx = context('inspect', '{"prompt":"absent"}', null);
  const output = await new Replant().execute.call(ctx);
  assert.equal(ctx.calls[0].url, 'http://replant:8799/inspect');
  assert.deepEqual(output[0][0].json, { record: null });
});

test('every input item evaluates its own parameters and preserves item linking', async () => {
  const ctx = context('run', '{}');
  const requests = [{ prompt: 'first' }, { prompt: 'second' }, { prompt: 'third' }];
  ctx.getInputData = () => requests.map((json) => ({ json }));
  ctx.getNodeParameter = (name, index) => name === 'operation' ? 'run' : requests[index];
  ctx.helpers.httpRequest = async (options) => {
    ctx.calls.push(options);
    return { status: 'succeeded', prompt: options.body.prompt };
  };
  const [items] = await new Replant().execute.call(ctx);
  assert.deepEqual(ctx.calls.map((call) => call.body), requests);
  assert.deepEqual(items.map((item) => item.json.prompt), requests.map((r) => r.prompt));
  assert.deepEqual(items.map((item) => item.pairedItem), [{ item: 0 }, { item: 1 }, { item: 2 }]);
});

test('empty batches perform no requests', async () => {
  const ctx = context('run', '{}');
  ctx.getInputData = () => [];
  assert.deepEqual(await new Replant().execute.call(ctx), [[]]);
  assert.equal(ctx.calls.length, 0);
});
