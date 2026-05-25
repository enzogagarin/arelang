use are_diagnostics::SourceRange;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    Use(UseDecl),
    Type(TypeDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Function(FunctionDecl),
    Service(ServiceDecl),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UseDecl {
    pub path: Path,
    pub alias: Option<String>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeDecl {
    pub name: String,
    pub aliased: TypeExpr,
    pub opaque: bool,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<Field>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    pub name: String,
    pub ty: TypeExpr,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<EnumVariant>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnumVariant {
    pub name: String,
    pub payload: Vec<Field>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: RawBlock,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RawBlock {
    pub token_count: usize,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceDecl {
    pub name: String,
    pub state_param: Option<Param>,
    pub uses: Vec<ServiceUse>,
    pub routes: Vec<RouteDecl>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceUse {
    pub target: Path,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteDecl {
    pub method: String,
    pub path: String,
    pub handler: Path,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Path {
    pub segments: Vec<String>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeExpr {
    Path {
        path: Path,
    },
    Generic {
        base: Path,
        args: Vec<TypeExpr>,
        range: SourceRange,
    },
    Option {
        inner: Box<TypeExpr>,
        range: SourceRange,
    },
}

impl TypeExpr {
    #[must_use]
    pub const fn range(&self) -> SourceRange {
        match self {
            Self::Path { path } => path.range,
            Self::Generic { range, .. } | Self::Option { range, .. } => *range,
        }
    }
}
