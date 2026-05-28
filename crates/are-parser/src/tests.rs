use super::parse_tokens;
use are_ast::{Expr, FieldValidation, FunctionBody, FunctionDecl, Item, Module, Stmt};
use are_lexer::lex_source;
use std::path::Path;

#[test]
fn parses_users_api_shape() {
    let source = include_str!("../../../examples/users_api/main.are");
    let file = Path::new("examples/users_api/main.are");
    let (tokens, lex_diagnostics) = lex_source(file, source);
    assert!(lex_diagnostics.is_empty());

    let (module, diagnostics) = parse_tokens(file, &tokens);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let module = module.expect("module parses");
    assert_eq!(module.items.len(), 25);
    assert!(matches!(module.items.last(), Some(Item::Service(_))));
    assert!(module.items.iter().any(|item| {
        matches!(
            item,
            Item::Model(model)
                if model.name == "User"
                    && model.fields.iter().any(|field| field.name == "id")
                    && model.fields.iter().any(|field| field.name == "email")
        )
    }));

    let health = function_named(&module, "health");
    let FunctionBody::Parsed { block } = &health.body else {
        panic!("health body should parse into a return block");
    };
    let Some(Stmt::Return { value, .. }) = block.statements.first() else {
        panic!("health should return a response");
    };
    assert!(matches!(value, Expr::Object { .. }));

    let create_user = function_named(&module, "create_user");
    let FunctionBody::Parsed { block } = &create_user.body else {
        panic!("create_user body should parse into statements");
    };
    assert_eq!(block.statements.len(), 2);
    assert!(matches!(block.statements.first(), Some(Stmt::Let { .. })));
    assert!(matches!(block.statements.last(), Some(Stmt::Return { .. })));

    let get_user = function_named(&module, "get_user");
    let FunctionBody::Parsed { block } = &get_user.body else {
        panic!("get_user body should parse into statements");
    };
    assert_eq!(block.statements.len(), 2);
    assert!(matches!(block.statements.first(), Some(Stmt::Let { .. })));
    assert!(matches!(block.statements.last(), Some(Stmt::Return { .. })));

    let api_error = module
        .items
        .iter()
        .find_map(|item| {
            if let Item::Enum(decl) = item {
                (decl.name == "ApiError").then_some(decl)
            } else {
                None
            }
        })
        .expect("ApiError enum parses");
    assert_eq!(api_error.variants[0].status.expect("status").value, 400);
    assert_eq!(api_error.variants[1].status.expect("status").value, 404);
    assert_eq!(api_error.variants[2].status.expect("status").value, 500);
}

#[test]
fn parses_field_validation_rules() {
    let source = r"
            type Email = opaque String validate.email
            type DisplayName = opaque String validate.length(min: 2, max: 80)

            struct CreateUserInput {
                email: Email validate.email
                name: String validate.length(min: 2, max: 80)
            }
        ";
    let file = Path::new("test.are");
    let (tokens, lex_diagnostics) = lex_source(file, source);
    assert!(lex_diagnostics.is_empty());

    let (module, diagnostics) = parse_tokens(file, &tokens);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let module = module.expect("module parses");
    let Some(Item::Type(email)) = module.items.first() else {
        panic!("expected Email type");
    };
    assert!(matches!(
        email.validations.as_slice(),
        [FieldValidation::Email { .. }]
    ));

    let Some(Item::Type(display_name)) = module.items.get(1) else {
        panic!("expected DisplayName type");
    };
    assert!(matches!(
        display_name.validations.as_slice(),
        [FieldValidation::Length {
            min: 2,
            max: 80,
            ..
        }]
    ));

    let Some(Item::Struct(input)) = module.items.get(2) else {
        panic!("expected struct");
    };

    assert!(matches!(
        input.fields[0].validations.as_slice(),
        [FieldValidation::Email { .. }]
    ));
    assert!(matches!(
        input.fields[1].validations.as_slice(),
        [FieldValidation::Length {
            min: 2,
            max: 80,
            ..
        }]
    ));
}

fn function_named<'a>(module: &'a Module, name: &str) -> &'a FunctionDecl {
    module
        .items
        .iter()
        .find_map(|item| {
            if let Item::Function(function) = item {
                (function.name == name).then_some(function)
            } else {
                None
            }
        })
        .expect("function exists")
}

#[test]
fn parses_service_routes() {
    let source = r#"
            service UsersApi(state: AppState) {
                use Http.error_map(map_error)
                route GET "/health" -> health returns HealthResponse status 200
                route POST "/users" -> create_user returns User status 201
            }
        "#;
    let file = Path::new("test.are");
    let (tokens, lex_diagnostics) = lex_source(file, source);
    assert!(lex_diagnostics.is_empty());

    let (module, diagnostics) = parse_tokens(file, &tokens);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let module = module.expect("module parses");
    let Some(Item::Service(service)) = module.items.first() else {
        panic!("expected service");
    };

    assert_eq!(service.routes.len(), 2);
    assert_eq!(service.uses.len(), 1);
    assert_eq!(service.uses[0].target.segments, ["Http", "error_map"]);
    assert_eq!(service.uses[0].args[0].segments, ["map_error"]);
    assert_eq!(service.routes[0].method, "GET");
    assert_eq!(service.routes[0].path, "/health");
    assert!(service.routes[0].response_type.is_some());
    assert_eq!(service.routes[0].status.expect("status").value, 200);
}

#[test]
fn parses_method_shorthand_route_contracts() {
    let source = r#"
            service UsersApi(state: AppState) {
                post "/users" body CreateUserInput -> create_user returns User status 201
                get "/users/search" query SearchUsersQuery -> search_users returns SearchUsersResponse status 200
                get "/users/auth-check" headers AuthHeaders -> auth_check returns AuthCheckResponse status 200
                get "/session" cookies SessionCookies -> current_session returns SessionResponse status 200
                get "/users/{id: UserId}" -> get_user returns User status 200
            }
        "#;
    let file = Path::new("test.are");
    let (tokens, lex_diagnostics) = lex_source(file, source);
    assert!(lex_diagnostics.is_empty());

    let (module, diagnostics) = parse_tokens(file, &tokens);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let module = module.expect("module parses");
    let Some(Item::Service(service)) = module.items.first() else {
        panic!("expected service");
    };

    assert_eq!(service.routes[0].method, "POST");
    assert_eq!(service.routes[0].path, "/users");
    assert!(service.routes[0].body_type.is_some());
    assert!(service.routes[0].query_type.is_none());
    assert!(service.routes[0].response_type.is_some());
    assert_eq!(service.routes[0].status.expect("status").value, 201);
    assert_eq!(service.routes[1].method, "GET");
    assert_eq!(service.routes[1].path, "/users/search");
    assert!(service.routes[1].body_type.is_none());
    assert!(service.routes[1].query_type.is_some());
    assert!(service.routes[1].headers_type.is_none());
    assert_eq!(service.routes[1].status.expect("status").value, 200);
    assert_eq!(service.routes[2].method, "GET");
    assert_eq!(service.routes[2].path, "/users/auth-check");
    assert!(service.routes[2].body_type.is_none());
    assert!(service.routes[2].query_type.is_none());
    assert!(service.routes[2].headers_type.is_some());
    assert_eq!(service.routes[2].status.expect("status").value, 200);
    assert_eq!(service.routes[3].method, "GET");
    assert_eq!(service.routes[3].path, "/session");
    assert!(service.routes[3].body_type.is_none());
    assert!(service.routes[3].query_type.is_none());
    assert!(service.routes[3].headers_type.is_none());
    assert!(service.routes[3].cookies_type.is_some());
    assert_eq!(service.routes[3].status.expect("status").value, 200);
    assert_eq!(service.routes[4].method, "GET");
    assert_eq!(service.routes[4].path, "/users/{id: UserId}");
    assert!(service.routes[4].body_type.is_none());
    assert!(service.routes[4].query_type.is_none());
    assert!(service.routes[4].headers_type.is_none());
    assert!(service.routes[4].cookies_type.is_none());
    assert_eq!(service.routes[4].status.expect("status").value, 200);
}

#[test]
fn rejects_duplicate_route_input_contracts() {
    let source = r#"
            service UsersApi(state: AppState) {
                post "/users" body CreateUserInput body OtherInput -> create_user returns User status 201
                get "/users/search" query SearchUsersQuery query OtherQuery -> search_users returns SearchUsersResponse status 200
                get "/users/auth-check" headers AuthHeaders headers OtherHeaders -> auth_check returns AuthCheckResponse status 200
                get "/session" cookies SessionCookies cookies OtherCookies -> current_session returns SessionResponse status 200
            }
        "#;
    let file = Path::new("test.are");
    let (tokens, lex_diagnostics) = lex_source(file, source);
    assert!(lex_diagnostics.is_empty());

    let (module, diagnostics) = parse_tokens(file, &tokens);

    assert!(module.is_none());
    assert_eq!(diagnostics.len(), 4, "{diagnostics:#?}");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "E_PARSE_0010")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.problem == "duplicate route body contract")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.problem == "duplicate route query contract")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.problem == "duplicate route headers contract")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.problem == "duplicate route cookies contract")
    );
}
