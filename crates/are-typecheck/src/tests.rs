use super::typecheck_module;
use are_lexer::lex_source;
use are_parser::parse_tokens;
use are_resolver::resolve_module;
use std::path::Path;

#[test]
fn typechecks_users_api_shape() {
    let source = include_str!("../../../examples/users_api/main.are");
    let diagnostics = diagnostics_for("examples/users_api/main.are", source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn rejects_bad_route_handler_context() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct OtherState {}

            fn bad(ctx: Http.Context<OtherState>, req: Http.Request) -> Http.Response {}

            service Api(state: AppState) {
                route GET "/bad" -> bad
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "E_HTTP_0202");
}

#[test]
fn requires_error_map_for_result_handlers() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            enum ApiError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {}

            service Api(state: AppState) {
                route POST "/users" -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "E_HTTP_0302");
}

#[test]
fn validates_error_map_signature() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            enum ApiError { Failed }
            enum OtherError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {}
            fn map_error(err: OtherError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                route POST "/users" -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "E_HTTP_0307");
}

#[test]
fn accepts_declarative_http_error_contracts() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            enum ApiError {
                Failed(message: String) status 500
                NotFound status 404
            }

            fn create(ctx: Http.Context<AppState>) -> Result<Http.Response, ApiError> {
                return Http.Response.ok({ "ok": true })
            }

            service Api(state: AppState) {
                use Http.errors(ApiError)
                post "/users" -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn validates_declarative_http_error_contracts() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            enum ApiError {
                Failed
                Redirect status 302
            }

            fn create(ctx: Http.Context<AppState>) -> Result<Http.Response, ApiError> {
                return Http.Response.ok({ "ok": true })
            }

            service Api(state: AppState) {
                use Http.errors(ApiError)
                post "/users" -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "E_HTTP_0311")
    );
}

#[test]
fn rejects_duplicate_struct_fields_and_params() {
    let source = r"
            struct User {
                id: U64
                id: U64
            }

            fn f(id: U64, id: U64) {}
        ";

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0].code, "E_TYPE_0004");
    assert_eq!(diagnostics[1].code, "E_TYPE_0003");
}

#[test]
fn rejects_duplicate_route_params() {
    let source = r#"
            use std.http as Http

            struct AppState {}

            fn get(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {}

            service Api(state: AppState) {
                route GET "/users/:id/orders/:id" -> get
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "E_HTTP_0104");
}

#[test]
fn checks_typed_route_params_against_handler_reads() {
    let source = r#"
            use std.http as Http

            type UserId = opaque U64
            type OrgId = opaque U64
            struct AppState {}
            enum ApiError { Failed }

            fn get(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let id = ctx.param<OrgId>("id")?
                return Http.Response.ok({ "id": id })
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                get "/users/{id: UserId}" -> get
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0402");
}

#[test]
fn accepts_route_contract_bound_handler_params() {
    let source = r#"
            use std.http as Http

            type UserId = opaque U64
            struct AppState {}
            struct CreateUserInput { email: String }
            struct SearchUsersQuery { email: String }
            struct AuthHeaders { authorization: String }
            struct SessionCookies { session_id: String }

            fn create(ctx: Http.Context<AppState>, input: CreateUserInput) -> Http.Response {
                return Http.Response.ok(input)
            }

            fn search(ctx: Http.Context<AppState>, query: SearchUsersQuery) -> Http.Response {
                return Http.Response.ok({ "email": query.email })
            }

            fn auth(ctx: Http.Context<AppState>, headers: AuthHeaders) -> Http.Response {
                return Http.Response.ok({ "authorization": headers.authorization })
            }

            fn session(ctx: Http.Context<AppState>, cookies: SessionCookies) -> Http.Response {
                return Http.Response.ok({ "session_id": cookies.session_id })
            }

            fn get(ctx: Http.Context<AppState>, id: UserId) -> Http.Response {
                return Http.Response.ok({ "id": id })
            }

            service Api(state: AppState) {
                post "/users" body CreateUserInput -> create
                get "/users/search" query SearchUsersQuery -> search
                get "/users/auth-check" headers AuthHeaders -> auth
                get "/session" cookies SessionCookies -> session
                get "/users/{id: UserId}" -> get
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn requires_body_contract_when_handler_decodes_json() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct CreateUserInput { name: String }
            enum ApiError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let input = req.json<CreateUserInput>()?
                return Http.Response.ok(input)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                post "/users" -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0414");
}

#[test]
fn checks_body_contract_against_handler_decode_type() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct CreateUserInput { name: String }
            struct OtherInput { name: String }
            enum ApiError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let input = req.json<OtherInput>()?
                return Http.Response.ok(input)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                post "/users" body CreateUserInput -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0413");
}

#[test]
fn requires_query_contract_when_handler_decodes_query() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct SearchUsersQuery { email: String }
            enum ApiError { Failed }

            fn search(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let query = req.query<SearchUsersQuery>()?
                return Http.Response.ok(query)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                get "/users/search" -> search
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0433");
}

#[test]
fn checks_query_contract_against_handler_decode_type() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct SearchUsersQuery { email: String }
            struct OtherQuery { name: String }
            enum ApiError { Failed }

            fn search(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let query = req.query<OtherQuery>()?
                return Http.Response.ok(query)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                get "/users/search" query SearchUsersQuery -> search
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0432");
}

#[test]
fn requires_headers_contract_when_handler_decodes_headers() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct AuthHeaders { authorization: String }
            enum ApiError { Failed }

            fn auth(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let headers = req.headers<AuthHeaders>()?
                return Http.Response.ok(headers)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                get "/users/auth-check" -> auth
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0443");
}

#[test]
fn checks_headers_contract_against_handler_decode_type() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct AuthHeaders { authorization: String }
            struct OtherHeaders { request_id: String }
            enum ApiError { Failed }

            fn auth(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let headers = req.headers<OtherHeaders>()?
                return Http.Response.ok(headers)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                get "/users/auth-check" headers AuthHeaders -> auth
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0442");
}

#[test]
fn requires_cookies_contract_when_handler_decodes_cookies() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct SessionCookies { session_id: String }
            enum ApiError { Failed }

            fn session(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let cookies = req.cookies<SessionCookies>()?
                return Http.Response.ok(cookies)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                get "/session" -> session
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0453");
}

#[test]
fn checks_cookies_contract_against_handler_decode_type() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct SessionCookies { session_id: String }
            struct OtherCookies { theme: String }
            enum ApiError { Failed }

            fn session(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let cookies = req.cookies<OtherCookies>()?
                return Http.Response.ok(cookies)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                get "/session" cookies SessionCookies -> session
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0452");
}

#[test]
fn checks_response_contract_against_handler_body() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct User { name: String }

            fn get_user(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {
                return Http.Response.ok("not_a_user")
            }

            service Api(state: AppState) {
                get "/users/1" -> get_user returns User status 200
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0422");
}

#[test]
fn checks_response_status_against_handler_success_status() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct User { name: String }

            fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {
                return Http.Response.ok({ "name": "Ada" })
            }

            service Api(state: AppState) {
                post "/users" -> create_user returns User status 201
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_HTTP_0423");
}

#[test]
fn accepts_domain_return_handlers_from_route_contracts() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct User { name: String }
            enum ApiError { Failed }

            fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<User, ApiError> {
                return { "name": "Ada" }
            }

            fn map_error(err: ApiError) -> Http.Response {
                return Http.Response.error(500, { "error": "failed" })
            }

            service Api(state: AppState) {
                use Http.error_map(map_error)
                post "/users" -> create_user returns User status 201
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn accepts_model_db_calls_for_non_user_collections() {
    let source = r#"
            use std.http as Http

            struct AppState {}
            struct CreatePostInput { title: String }
            model Post {
                id: U64 primary
                title: String
            }
            enum ApiError { Failed }

            fn create_post(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Post, ApiError> {
                let input = req.json<CreatePostInput>()?
                let post = ctx.db.posts.insert(input)?
                return post
            }

            fn map_error(err: ApiError) -> Http.Response {
                return Http.Response.error(500, { "error": "failed" })
            }

            service Api(state: AppState) {
                use Http.error_map(map_error)
                post "/posts" body CreatePostInput -> create_post returns Post status 201
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn rejects_invalid_result_arity() {
    let source = r#"
            use std.http as Http

            struct AppState {}

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response> {}

            service Api(state: AppState) {
                route POST "/users" -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0].code, "E_HTTP_0204");
    assert_eq!(diagnostics[1].code, "E_TYPE_0002");
}

#[test]
fn rejects_unknown_fields_in_function_bodies() {
    let source = r#"
            use std.http as Http
            use std.validate

            type UserId = opaque U64

            struct AppState { users: UserStore }
            struct UserStore {}
            struct CreateUserInput {
                email: String
                name: String
            }
            struct User {
                id: UserId
                email: String
                name: String
            }
            enum ApiError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let input = req.json<CreateUserInput>()?
                ensure validate.email(input.emali), ApiError.Failed
                return Http.Response.created(input)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                post "/users" body CreateUserInput -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_BODY_0002");
    assert_eq!(diagnostics[0].fixes[0].label, "did you mean `email`?");
}

#[test]
fn rejects_question_on_non_result_values() {
    let source = r#"
            use std.http as Http

            struct AppState {}

            fn ping(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {
                let response = Http.Response.ok({ "message": "pong" })?
                return response
            }

            service Api(state: AppState) {
                route GET "/ping" -> ping
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_BODY_0005");
}

#[test]
fn rejects_invalid_builtin_named_argument_types() {
    let source = r#"
            use std.http as Http
            use std.validate

            struct AppState {}
            struct CreateUserInput { name: String }
            enum ApiError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let input = req.json<CreateUserInput>()?
                ensure validate.length(input.name, min: "two", max: 80), ApiError.Failed
                return Http.Response.ok(input)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                post "/users" body CreateUserInput -> create
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_BODY_0006");
}

#[test]
fn checks_declarative_field_validation_rules() {
    let source = r"
            type Email = opaque String validate.email
            type DisplayName = opaque String validate.length(min: 2, max: 80)
            type BadId = opaque U64 validate.email
            type BadTitle = opaque String validate.length(min: 10, max: 2)

            struct Good {
                email: Email
                name: DisplayName
            }

            struct Bad {
                id: U64 validate.email
                title: String validate.length(min: 10, max: 2)
            }
        ";

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 4, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_TYPE_0010");
    assert_eq!(diagnostics[1].code, "E_TYPE_0011");
    assert_eq!(diagnostics[2].code, "E_TYPE_0010");
    assert_eq!(diagnostics[3].code, "E_TYPE_0011");
}

#[test]
fn rejects_invalid_local_function_argument_types() {
    let source = r#"
            struct CreateUserInput { name: String }
            enum ApiError { Failed }

            fn validate_user(input: CreateUserInput) -> Result<CreateUserInput, ApiError> {
                return input
            }

            fn bad() -> Result<CreateUserInput, ApiError> {
                return validate_user("Ada")?
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_BODY_0006");
}

#[test]
fn rejects_non_exhaustive_enum_match() {
    let source = r#"
            use std.http as Http

            enum ApiError {
                InvalidInput(message: String)
                NotFound
            }

            fn map_error(err: ApiError) -> Http.Response {
                match err {
                    InvalidInput(message) => return Http.Response.error(400, { "error": message })
                }
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_BODY_0011");
}

#[test]
fn rejects_ensure_with_non_bool_condition() {
    let source = r#"
            enum ApiError { Failed }

            fn validate() -> Result<String, ApiError> {
                ensure "yes", ApiError.Failed
                return "ok"
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_BODY_0006");
}

#[test]
fn rejects_invalid_enum_constructor_payloads() {
    let source = r#"
            enum ApiError {
                InvalidInput(message: String)
            }

            fn validate() -> Result<String, ApiError> {
                ensure false, ApiError.InvalidInput(400)
                return "ok"
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_BODY_0006");
}

#[test]
fn suggests_nearby_enum_variants() {
    let source = r#"
            enum ApiError {
                InvalidInput(message: String)
            }

            fn validate() -> Result<String, ApiError> {
                ensure false, ApiError.InvaldInput("bad")
                return "ok"
            }
        "#;

    let diagnostics = diagnostics_for("test.are", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].code, "E_BODY_0012");
    assert_eq!(
        diagnostics[0].fixes[0].label,
        "did you mean `InvalidInput`?"
    );
}

fn diagnostics_for(file_name: &str, source: &str) -> Vec<are_diagnostics::Diagnostic> {
    let file = Path::new(file_name);
    let (tokens, lex_diagnostics) = lex_source(file, source);
    assert!(lex_diagnostics.is_empty(), "{lex_diagnostics:#?}");

    let (module, parse_diagnostics) = parse_tokens(file, &tokens);
    assert!(parse_diagnostics.is_empty(), "{parse_diagnostics:#?}");
    let module = module.expect("module parses");

    let resolve_diagnostics = resolve_module(file, &module);
    let mut diagnostics = resolve_diagnostics;
    diagnostics.extend(typecheck_module(file, &module));
    diagnostics
}
