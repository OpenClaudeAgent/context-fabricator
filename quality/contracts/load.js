import http from 'k6/http';
import { check, sleep } from 'k6';

export const options = {
  vus: 3,
  iterations: 60,
  thresholds: {
    checks: ['rate==1'],
    http_req_duration: [__ENV.CTXFAB_IMPOSSIBLE === '1' ? 'p(95)<0' : 'p(95)<500'],
  },
};

export default function () {
  const response = http.get(`${__ENV.CTXFAB_TEST_URL}/v1/status`, {
    headers: { Authorization: `Bearer ${__ENV.CTXFAB_TEST_TOKEN}` },
  });
  check(response, {
    'authenticated status is ready': (result) =>
      result.status === 200 && result.json('status') === 'ready',
  });
  sleep(0.02);
}
