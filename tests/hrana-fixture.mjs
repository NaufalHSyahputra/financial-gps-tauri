import { DatabaseSync } from 'node:sqlite';

export function hranaFixture(origin, path, tokens) {
  const cloud = new DatabaseSync(path);
  const counts = { requests: 0, probes: 0, networkDisabled: false };
  const access = {
    'access-control-allow-origin': origin,
    'access-control-allow-methods': 'POST, OPTIONS',
    'access-control-allow-headers': 'authorization, content-type',
    'content-type': 'application/json'
  };
  const decode = value => value.type === 'integer' ? BigInt(value.value)
    : value.type === 'blob' ? Buffer.from(value.base64, 'base64') : value.value ?? null;
  const encode = value => value === null ? { type: 'null' }
    : typeof value === 'bigint' ? { type: 'integer', value: String(value) }
      : value instanceof Uint8Array ? { type: 'blob', base64: Buffer.from(value).toString('base64').replace(/=+$/, '') }
        : { type: 'text', value };

  async function endpoint(route) {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill({ status: 204, headers: access, body: '' });
    counts.requests++;
    if (counts.networkDisabled) return route.fulfill({ status: 503, headers: access, body: '{}' });
    if (!tokens.has(request.headers().authorization?.slice('Bearer '.length))) return route.fulfill({ status: 401, headers: access, body: '{}' });
    try {
      const [{ stmt }] = request.postDataJSON().requests;
      if (stmt.sql.trim() === 'SELECT 1') counts.probes++;
      const args = stmt.args.map(decode);
      const indexed = [];
      const sql = stmt.sql.replace(/\?(\d+)/g, (_, index) => {
        indexed.push(args[Number(index) - 1]);
        return '?';
      });
      const query = cloud.prepare(sql);
      query.setReadBigInts(true);
      let result;
      if (query.columns().length) {
        query.setReturnArrays(true);
        result = { rows: query.all(...indexed).map(row => row.map(encode)), affected_row_count: 0 };
      } else {
        result = { rows: [], affected_row_count: Number(query.run(...indexed).changes) };
      }
      return route.fulfill({ status: 200, headers: access, json: { results: [{ type: 'ok', response: { type: 'execute', result } }, { type: 'ok', response: { type: 'close' } }] } });
    } catch (error) {
      return route.fulfill({ status: 200, headers: access, json: { results: [{ type: 'error', error: { code: 'SQLITE_ERROR', message: String(error) } }] } });
    }
  }
  return { endpoint, counts, close: () => cloud.close() };
}
