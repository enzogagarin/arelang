use super::parse_tokens;
use are_ast::{Expr, FunctionBody, FunctionDecl, Item, Module, Stmt};
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
    assert_eq!(module.items.len(), 15);
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
    assert!(matches!(value, Expr::Call { .. }));

    let validate_user = function_named(&module, "validate_user");
    let FunctionBody::Parsed { block } = &validate_user.body else {
        panic!("validate_user body should parse into statements");
    };
    assert_eq!(block.statements.len(), 3);
    assert!(matches!(
        block.statements.first(),
        Some(Stmt::Ensure { .. })
    ));

    let create_user = function_named(&module, "create_user");
    let FunctionBody::Parsed { block } = &create_user.body else {
        panic!("create_user body should parse into statements");
    };
    assert_eq!(block.statements.len(), 3);
    assert!(matches!(block.statements.first(), Some(Stmt::Let { .. })));
    assert!(matches!(block.statements.last(), Some(Stmt::Return { .. })));

    let get_user = function_named(&module, "get_user");
    let FunctionBody::Parsed { block } = &get_user.body else {
        panic!("get_user body should parse into statements");
    };
    assert_eq!(block.statements.len(), 3);
    assert!(matches!(block.statements.first(), Some(Stmt::Let { .. })));
    assert!(matches!(block.statements.last(), Some(Stmt::Return { .. })));

    let map_error = function_named(&module, "map_error");
    let FunctionBody::Parsed { block } = &map_error.body else {
        panic!("map_error body should parse into a match statement");
    };
    assert!(matches!(
        block.statements.first(),
        Some(Stmt::Match { arms, .. }) if arms.len() == 3
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
    assert!(service.routes[0].response_type.is_some());
    assert_eq!(service.routes[0].status.expect("status").value, 201);
    assert_eq!(service.routes[1].method, "GET");
    assert_eq!(service.routes[1].path, "/users/{id: UserId}");
    assert!(service.routes[1].body_type.is_none());
    assert_eq!(service.routes[1].status.expect("status").value, 200);
}
