# Arelang Syntax Seed

This is the first syntax seed for the HTTP MVP. It is intentionally small.

## Files

- Source files use `.are`
- Package manifests use `are.toml`
- Formatting is canonical and will be handled by `are fmt`

## Style

- Brace-based blocks
- No semicolons
- Newlines separate statements
- Public APIs must write types explicitly
- Local variables may use inference
- Mutation must be visible

```are
let name = "Ada"
let mut count = 0
```

## Functions

```are
fn health(ctx: Http.Context<AppState>, req: Http.Request) -> HealthResponse {
    return { "status": "ok" }
}
```

Functions use explicit return types. A missing return type means `Unit` only if the function is private and the compiler can prove it.

## Errors

v0 uses explicit `Result<T, E>`.

```are
fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<User, ApiError> {
    let input = req.json<CreateUserInput>()?
    let user = ctx.db.users.insert(input)?
    return user
}
```

`?` may only be used inside a function returning `Result`.

## Absence

v0 uses explicit `Option<T>`.

```are
fn find_user(id: UserId) -> Option<User> {
    return users.get(id)
}
```

Null is not part of the language.

## Structs

```are
struct User {
    id: UserId
    email: Email
    name: String
}
```

## Enums

```are
enum ApiError {
    InvalidInput(message: String)
    NotFound
    Internal(message: String)
}
```

## Services And Routes

`service` and `route` are allowed in v0 because HTTP is the first product target.

```are
service UsersApi(state: AppState) {
    get "/health" -> health returns HealthResponse status 200
    post "/users" body CreateUserInput -> create_user returns User status 201
    get "/users/search" query SearchUsersQuery -> search_users returns SearchUsersResponse status 200
    get "/users/{id: UserId}" -> get_user returns User status 200
}
```

The compiler builds a route registry from this declaration. Method shorthand is the canonical style, while the older `route GET "/path" -> handler` shape remains parseable during the MVP transition. Body contracts, query contracts, response contracts, status contracts, and typed path parameters are checked against handler code: `body CreateUserInput` must line up with `req.json<CreateUserInput>()`, `query SearchUsersQuery` must line up with `req.query<SearchUsersQuery>()`, `returns User status 201` must line up with the success response where the compiler can infer it, and `{id: UserId}` must line up with `ctx.param<UserId>("id")`.

## v0 Keywords

```text
use
as
pub
fn
let
mut
struct
enum
type
opaque
if
else
while
for
in
return
break
continue
match
service
route
test
unsafe
foreign
true
false
```

New keywords should be rare. If a feature can start as a standard library API, it should start there.
