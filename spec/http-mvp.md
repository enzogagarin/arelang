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
- fetches users with `GET /users/:id`
- uses JSON request and response bodies
- returns structured API errors

Current implementation status:

- `are run examples/users_api` starts a real local server
- route registry comes from parsed and typechecked Arelang service declarations
- function bodies are not interpreted yet
- users API behavior is implemented by a temporary runtime adapter

## Handler Shape

Handlers receive a typed context and request.

```are
fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError>
```

The first type checker accepts either:

```are
fn health(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response
fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError>
```

Routes returning `Result<Http.Response, E>` require one service-level mapper:

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
    route GET "/health" -> health
    route POST "/users" -> create_user
    route GET "/users/:id" -> get_user
}
```

The compiler should check:

- handler exists
- handler has a valid HTTP signature
- handler returns `Http.Response` or `Result<Http.Response, E>`
- result-returning handlers have a compatible `Http.error_map`
- route params are available to the handler through `ctx.param<T>("name")`
- duplicate method/path pairs are rejected

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
