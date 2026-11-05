export class ReplantError extends Error {
  constructor(message, status, receipt) {
    super(message);
    this.name = 'ReplantError';
    this.status = status;
    this.receipt = receipt;
  }
}

export class ReplantClient {
  constructor(baseUrl, token, options = {}) {
    if (!baseUrl) throw new TypeError('baseUrl is required');
    if (!token) throw new TypeError('token is required');
    this.baseUrl = baseUrl.replace(/\/+$/, '');
    this.token = token;
    this.fetchImpl = options.fetchImpl || globalThis.fetch;
    this.timeoutMs = options.timeoutMs ?? 30_000;
  }

  async _request(method, path, body) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
        method,
        headers: {
          Authorization: `Bearer ${this.token}`,
          'Content-Type': 'application/json',
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: controller.signal,
      });
      const text = await response.text();
      let value;
      try {
        value = text ? JSON.parse(text) : null;
      } catch {
        value = text;
      }
      if (!response.ok) {
        throw new ReplantError(`replant ${method} ${path}: HTTP ${response.status}`, response.status, value);
      }
      return value;
    } finally {
      clearTimeout(timer);
    }
  }

  health() {
    return this._request('GET', '/healthz');
  }

  run(request) {
    return this._request('POST', '/run', request);
  }

  record(record) {
    return this._request('POST', '/record', record);
  }

  inspect(request) {
    return this._request('POST', '/inspect', request);
  }

  stats() {
    return this._request('GET', '/stats');
  }

  approve(request, candidate) {
    return this._request('POST', '/approve', { request, candidate });
  }

  reject(request, candidate) {
    return this._request('POST', '/reject', { request, candidate });
  }

  undo(id) {
    return this._request('POST', '/undo', { id });
  }
}
