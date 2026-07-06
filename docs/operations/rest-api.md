# REST API

The REST server is exposed by:

```sh
cargo run -p quarry -- server start --addr 127.0.0.1:7831
```

Implemented phase-one core endpoints:

- `POST /v1/libraries`
- `GET /v1/libraries`
- `GET /v1/libraries/{library}`
- `GET /v1/libraries/{library}/documents?prefix=&limit=`
- `GET /v1/libraries/{library}/documents/{path}`
- `HEAD /v1/libraries/{library}/documents/{path}`
- `PUT /v1/libraries/{library}/documents/{path}`
- `PATCH /v1/libraries/{library}/documents/{path}/metadata`
- `POST /v1/libraries/{library}/documents/{path}/move`
- `DELETE /v1/libraries/{library}/documents/{path}`
- `POST /v1/libraries/{library}/transactions`
- `PUT /v1/libraries/{library}/transactions/{tx}/documents/{path}`
- `PATCH /v1/libraries/{library}/transactions/{tx}/documents/{path}/metadata`
- `POST /v1/libraries/{library}/transactions/{tx}/documents/{path}/move`
- `DELETE /v1/libraries/{library}/transactions/{tx}/documents/{path}`
- `POST /v1/libraries/{library}/transactions/{tx}/commit`
- `POST /v1/libraries/{library}/transactions/{tx}/rollback`
- `POST /v1/libraries/{library}/git/peers`
- `GET /v1/libraries/{library}/git/peers`
- `POST /v1/libraries/{library}/git/import`
- `POST /v1/libraries/{library}/git/export`
- `POST /v1/libraries/{library}/git/peers/{peer}/pull`
- `POST /v1/libraries/{library}/git/peers/{peer}/push`
- `POST /v1/libraries/{library}/git/peers/{peer}/sync`
- `GET /v1/libraries/{library}/conflicts`
- `GET /v1/libraries/{library}/conflicts/{conflict}`
- `POST /v1/libraries/{library}/conflicts/{conflict}/resolve`
- `POST /v1/admin/gc`
- `GET /v1/health`
- `GET /v1/openapi.json`

Document reads return an `ETag` based on the visible document version. Writes support `If-Match` and `If-None-Match: *`. Explicit transaction commits return `412 Precondition Failed` if any staged document head changed before commit, leaving the newer committed document visible.
