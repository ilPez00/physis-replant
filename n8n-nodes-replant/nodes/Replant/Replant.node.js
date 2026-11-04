'use strict';

const OPERATIONS = {
  run: { method: 'POST', path: '/run' },
  record: { method: 'POST', path: '/record' },
  inspect: { method: 'POST', path: '/run' },
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
            'run/inspect: {prompt,input,context}; record: {request,recipe,trace}; approve/reject: {request,candidate}; undo: {id}',
        },
      ],
    };
  }

  async execute() {
    const credentials = await this.getCredentials('replantApi');
    const operation = this.getNodeParameter('operation', 0, 'run');
    const spec = OPERATIONS[operation];
    if (!spec) {
      throw new Error(`Unknown Replant operation: ${operation}`);
    }
    const baseUrl = String(credentials.baseUrl || '').replace(/\/+$/, '');
    if (!baseUrl) {
      throw new Error('Replant Base URL is required');
    }
    let payload;
    const raw = this.getNodeParameter('payload', 0, '{}');
    try {
      payload = typeof raw === 'string' ? JSON.parse(raw) : raw;
    } catch (error) {
      throw new Error(`Replant payload is not valid JSON: ${error.message}`);
    }
    const response = await this.helpers.httpRequest({
      method: spec.method,
      url: `${baseUrl}${spec.path}`,
      headers: {
        Authorization: `Bearer ${credentials.token || ''}`,
        'Content-Type': 'application/json',
      },
      body: spec.method === 'GET' ? undefined : payload,
      json: true,
    });
    return [[{ json: response }]];
  }
}

module.exports = { Replant };
