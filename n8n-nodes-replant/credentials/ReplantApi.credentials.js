'use strict';

class ReplantApi {
  constructor() {
    this.name = 'replantApi';
    this.displayName = 'Replant API';
    this.documentationUrl = 'https://github.com/ilPez00/physis-replant';
    this.properties = [
      {
        displayName: 'Base URL',
        name: 'baseUrl',
        type: 'string',
        default: 'http://172.17.0.1:8799',
        required: true,
      },
      {
        displayName: 'Bearer Token',
        name: 'token',
        type: 'string',
        typeOptions: { password: true },
        default: '',
        required: true,
      },
    ];
    this.authenticate = {
      type: 'generic',
      properties: {
        headers: {
          Authorization: '=Bearer {{$credentials.token}}',
        },
      },
    };
    this.test = {
      request: {
        baseURL: '={{$credentials.baseUrl}}',
        url: '/healthz',
        method: 'GET',
      },
    };
  }
}

module.exports = { ReplantApi };
