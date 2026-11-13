# API tests without a browser

A scenario whose steps never touch a page (`http`, `exec`, their checks,
checks of saved values and of cookies, `save … = json|header`, `print` of
an answer, `retry`, `wait --ms`, a `flow` or an `auth` made of those) runs
with no browser at all, and a run of only such scenarios starts none:
nothing to install on the machine, and each request costs about a
millisecond on a kept-alive connection.

```text
base http://127.0.0.1:8000

auth api
  http POST /api/login --form user=ada --form "password=${secret:PW}"

scenario "An order can be placed and ships" @api
  as api
  http POST /api/orders --json '{"sku": "sku-1", "qty": 2}'
    expect status 201
    expect json $ schema order.schema.json
    expect time < 500ms
  save order = json $.id
  retry --timeout-ms 20000
    http GET /api/orders/${order}
      expect json $ contains '{"status": "SHIPPED", "items": [{"sku": "sku-1"}]}'
```

```console
$ bitti run tests/api
PASS  tests/api/orders.bt:6  An order can be placed and ships  (412 ms)
1 passed, 0 failed in 415 ms (no browser)
```

- Such a scenario keeps its cookies the way a browser would (domain, path,
  `Secure`, expiry; a cookie set on a redirect goes with the next hop):
  what its sign-in sets goes with its later requests, and `expect cookie
  sid` checks one.
- An `auth` of `http` steps signs in with no browser too, and saves its
  cookies in navigera's `state save` format: a browser scenario `as` the
  same auth starts signed in, and an API scenario `as` an auth that signed
  in through the page takes its cookies.
- A failure shows the request and what came back:

  ```text
  FAIL  orders.bt:6  An order can be placed and ships  (31 ms)
        line 10: expect json $ schema order.schema.json
        expected json $ to match schema order.schema.json; it doesn't: $.total: expected number, got string "36.00"
        last http: POST http://127.0.0.1:8000/api/orders -> 201 (9 ms, 61 B, application/json)
          {"id": "ORD-7", "status": "NEW", "total": "36.00", "items": [...]}
  ```
- `--trace` records each step with the requests it made, and `--html`,
  `--junit`, `--allure` report these scenarios as any other.
- The schema check reads what an API's contract uses, in drafts 4 to
  2020-12 alike: `type` (and OpenAPI 3.0's `nullable`), `enum`, `const`,
  `properties`, `required`, `additionalProperties`, `patternProperties`,
  `propertyNames`, `min/maxProperties`, `dependentRequired`, `items` (or a
  list of them), `prefixItems`, `contains`, `min/maxItems`, `uniqueItems`,
  `min/maxLength`, `pattern`, `format` (`date-time`, `date`, `time`,
  `email`, `uuid`, `uri`, `ipv4`, `ipv6`), `minimum`/`maximum` and their
  exclusive forms, `multipleOf`, `allOf`, `anyOf`, `oneOf`, `not`,
  `if`/`then`/`else`, and `$ref` within the file (`#/$defs/…`,
  `#/definitions/…`). Other keywords are ignored, as validators do.
