# HTTP MVP Spec

The first Arelang version must run a real HTTP server.

## Goal

Run:

```sh
are run examples/users_api
```

Expected result:

- starts a local server
- exposes `/health`
- creates users with `POST /users`
- searches users with typed query params through `GET /users/search?email=...`
- checks typed request headers through `GET /users/auth-check`
- checks typed request cookies through `GET /session`
- fetches users with `GET /users/{id: UserId}`
- uses JSON request and response bodies
- returns structured API errors

Current implementation status:

- `are run examples/users_api` starts a real local server
- route registry comes from parsed and typechecked Arelang service declarations
- runtime preparation builds one checked HTTP contract manifest for service, routes, body types, query types, headers types, cookies types, response types, statuses, typed params, handlers, local schemas, and the error mapper
- `are inspect --json` exposes that checked HTTP contract manifest without starting the server, including aliases, structs, models, enum variants, model collections, and model field metadata
- `are openapi` exports the checked HTTP contract manifest as OpenAPI 3.1 JSON with paths, request bodies, responses, typed path/query/header/cookie parameters, server URL, component schemas, file output, and drift checks
- `are audit --json` checks route contracts and `[capabilities]` against the HTTP server surface
- canonical service syntax supports `get`, `post`, typed path params, request body contracts, request query contracts, request headers contracts, request cookies contracts, response contracts, and success status contracts
- incoming requests and outgoing responses pass through explicit MVP runtime request/response types
- route handlers execute through the MVP Arelang function-body interpreter
- `are new --template users` creates a runnable backend-first users API project
- `are run` prints the service URL and route table before accepting requests
- `ctx.db.<collection>.insert/get` is backed by local `model` declarations in the MVP in-memory store

## Handler Shape

Handlers receive a typed context and request.

```are
fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<User, ApiError>
```

The preferred handler style returns domain payloads and lets the route contract carry HTTP status:

```are
fn health(ctx: Http.Context<AppState>, req: Http.Request) -> HealthResponse
fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<User, ApiError>
```

The compatibility style still accepts explicit HTTP responses:

```are
fn health(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response
fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError>
```

Routes returning `Result<Payload, E>` or `Result<Http.Response, E>` require one service-level mapper:

```are
fn map_error(err: ApiError) -> Http.Response

service UsersApi(state: AppState) {
    use Http.error_map(map_error)
}
```

The context carries:

- app state
- request id
- logger
- route params
- future request-scope region data

## Route Declaration

```are
service UsersApi(state: AppState) {
    get "/health" -> health returns HealthResponse status 200
    post "/users" body CreateUserInput -> create_user returns User status 201
    get "/users/search" query SearchUsersQuery -> search_users returns SearchUsersResponse status 200
    get "/users/auth-check" headers AuthHeaders -> auth_check returns AuthCheckResponse status 200
    get "/session" cookies SessionCookies -> current_session returns SessionResponse status 200
    get "/users/{id: UserId}" -> get_user returns User status 200
}
```

The compiler should check:

- handler exists
- handler has a valid HTTP signature
- handler returns `Http.Response`, `Result<Http.Response, E>`, the declared `returns` payload, or `Result<Payload, E>`
- result-returning handlers have a compatible `Http.error_map`
- typed route params such as `{id: UserId}` are read through matching `ctx.param<UserId>("id")`
- body contracts such as `body CreateUserInput` are decoded through matching `req.json<CreateUserInput>()`
- query contracts such as `query SearchUsersQuery` are decoded through matching `req.query<SearchUsersQuery>()`
- headers contracts such as `headers AuthHeaders` are decoded through matching `req.headers<AuthHeaders>()`
- cookies contracts such as `cookies SessionCookies` are decoded through matching `req.cookies<SessionCookies>()`
- response contracts such as `returns User` name a known JSON payload type and match handler domain return types
- status contracts such as `status 201` use valid HTTP status codes and match explicit success response constructors when a handler still returns `Http.Response`
- model database calls such as `ctx.db.users.insert(input)` resolve `users` from `model User`
- duplicate method/path pairs are rejected

At runtime, the checked HTTP contract manifest is the source of truth for route matching, request body/query/header/cookie validation, domain payload wrapping, success response validation, and tool-facing API schema export. The OpenAPI exporter consumes that same manifest, so documentation/client generation follows the exact route contracts the runtime uses. Domain payloads are wrapped into HTTP responses with the route `status` value, then successful responses are validated against `returns` and `status` before they leave the HTTP boundary. Error responses produced by `Http.error_map` are intentionally outside the success response contract.

## Response Helpers

The v0 HTTP module should expose:

```are
Http.Response.ok(value)
Http.Response.created(value)
Http.Response.json(status, value)
Http.Response.error(status, value)
```

## JSON

v0 should support typed decode and encode for structs with primitive fields.

```are
let input = req.json<CreateUserInput>()?
let query = req.query<SearchUsersQuery>()?
let headers = req.headers<AuthHeaders>()?
let cookies = req.cookies<SessionCookies>()?
```

Supported primitive JSON mappings:

- `String`
- `Bool`
- `I64`
- `U64`
- `F64`
- structs
- `Option<T>`

Arrays can come after the first server works.

## Validation

Validation should be present in the users API, but it may be implemented as simple standard library functions before attributes exist.

```are
validate.email(input.email)?
validate.length(input.name, min: 2, max: 80)?
```

Attribute validation can come later:

```are
email: Email @validate.email
```

## Error Mapping

An API error enum maps to HTTP status codes through one function.

```are
fn map_error(err: ApiError) -> Http.Response
```

This is simpler than a magical framework mapper for v0 and easier for the compiler to support.

## Capability Audit

HTTP server projects declare the first MVP capability surface in `are.toml`:

```toml
[capabilities]
network_listen = ["127.0.0.1:8080"]
network_outbound = []
filesystem_read = []
filesystem_write = []
env_read = []
process_spawn = false
```

`are audit` verifies that the declared listen capability matches `[server]`, that route success contracts are explicit, and that unused network, filesystem, environment, and process capabilities stay closed by default.
