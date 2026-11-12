'use strict';

const OPERATIONS = {
  run: { method: 'POST', path: '/run' },
  record: { method: 'POST', path: '/record' },
  rerecord: { method: 'POST', path: '/rerecord' },
  inspect: { method: 'POST', path: '/inspect' },
  stats: { method: 'GET', path: '/stats' },
  approve: { method: 'POST', path: '/approve' },
  reject: { method: 'POST', path: '/reject' },
  undo: { method: 'POST', path: '/undo' },
};

class Replant {
  constructor() {
    this.description = {
      displayName: 'Replant',
      name: 'replant',
      group: ['transform'],
      version: 1,
      description:
        'Look up, record, review or undo verified tool recipes without repeating inference',
      defaults: { name: 'Replant' },
      inputs: ['main'],
      outputs: ['main'],
      credentials: [{ name: 'replantApi', required: true }],
      properties: [
        {
          displayName: 'Operation',
          name: 'operation',
          type: 'options',
          options: [
            { name: 'Run', value: 'run' },
            { name: 'Record', value: 'record' },
            { name: 'Re-record (Repeat Effects)', value: 'rerecord' },
            { name: 'Inspect', value: 'inspect' },
            { name: 'Stats', value: 'stats' },
            { name: 'Approve Near-Match', value: 'approve' },
            { name: 'Reject Near-Match', value: 'reject' },
            { name: 'Undo', value: 'undo' },
          ],
          default: 'run',
        },
        {
          displayName: 'Payload (JSON)',
          name: 'payload',
          type: 'json',
          default: '{}',
          description:
            'run/inspect: {prompt,input,context}; record: {request,recipe,trace}; approve/reject: {request,candidate}; rerecord/undo: {id}. Re-record repeats real effects; never use it automatically after a stale result or timeout.',
        },
        {
          displayName: 'Request Timeout (Milliseconds)',
          name: 'requestTimeout',
          type: 'number',
          typeOptions: { minValue: 1, maxValue: 86_400_000, numberPrecision: 0 },
          default: 30_000,
          description:
            'Set above the entire authorized recipe duration for long jobs. A timeout does not prove the effects were cancelled; inspect before retrying.',
        },
      ],
    };
  }

  async execute() {
    const credentials = await this.getCredentials('replantApi');
    const baseUrl = String(credentials.baseUrl || '').replace(/\/+$/, '');
    if (!baseUrl) {
      throw new Error('Replant Base URL is required');
    }
    const items = this.getInputData();
    const output = [];
    for (let index = 0; index < items.length; index++) {
      const operation = this.getNodeParameter('operation', index, 'run');
      const spec = Object.hasOwn(OPERATIONS, operation) ? OPERATIONS[operation] : undefined;
      if (!spec) {
        throw new Error(`Unknown Replant operation: ${operation}`);
      }
      const timeout = this.getNodeParameter('requestTimeout', index, 30_000);
      if (!Number.isInteger(timeout) || timeout < 1 || timeout > 86_400_000) {
        throw new Error('Replant request timeout must be an integer in 1..86400000 milliseconds');
      }
      let payload;
      if (spec.method !== 'GET') {
        const raw = this.getNodeParameter('payload', index, '{}');
        try {
          payload = typeof raw === 'string' ? JSON.parse(raw) : raw;
        } catch (error) {
          throw new Error(`Replant payload is not valid JSON: ${error.message}`);
        }
      }
      const response = await this.helpers.httpRequest({
        method: spec.method,
        url: `${baseUrl}${spec.path}`,
        headers: {
          Authorization: `Bearer ${credentials.token || ''}`,
          'Content-Type': 'application/json',
        },
        body: payload,
        json: true,
        timeout,
      });
      output.push({
        json: operation === 'inspect' ? { record: response } : response,
        pairedItem: { item: index },
      });
    }
    return [output];
  }
}

module.exports = { Replant };
