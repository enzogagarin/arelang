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
fn health() -> Http.Response {
    return Http.Response.ok({ "status": "ok" })
}
```

Functions use explicit return types. A missing return type means `Unit` only if the function is private and the compiler can prove it.

## Errors

v0 uses explicit `Result<T, E>`.

```are
fn create_user(req: Http.Request) -> Result<Http.Response, ApiError> {
    let input = req.json<CreateUserInput>()?
    return Http.Response.created(input)
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
    route GET "/health" -> health
    route POST "/users" -> create_user
    route GET "/users/:id" -> get_user
}
```

The compiler should build a route registry from this declaration.

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
