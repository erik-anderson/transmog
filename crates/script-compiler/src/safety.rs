//! Bound recursive AST work and dispose rejected trees without recursive Drop.

use swc_common::{DUMMY_SP, util::take::Take};
use swc_ecma_ast::{
    Expr, Ident, JSXElement, JSXFragment, Module, Pat, Stmt, TsEntityName, TsKeywordType,
    TsKeywordTypeKind, TsModuleBlock, TsNamespaceBody, TsType,
};
use swc_ecma_visit::{Visit, VisitMut, VisitMutWith, VisitWith};

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 100_000;

pub(super) fn validate(module: &Module) -> Result<(), String> {
    let mut budget = Budget {
        depth: 0,
        nodes: 0,
        exceeded: false,
    };
    module.visit_with(&mut budget);
    if budget.exceeded {
        Err(super::COMPLEXITY_LIMIT.into())
    } else {
        Ok(())
    }
}

struct Budget {
    depth: usize,
    nodes: usize,
    exceeded: bool,
}

macro_rules! bounded_nodes {
    ($($method:ident : $node:ty),+ $(,)?) => {$ (
        fn $method(&mut self, node: &$node) {
            if self.exceeded { return; }
            self.nodes += 1;
            self.depth += 1;
            if self.nodes > MAX_NODES || self.depth > MAX_DEPTH { self.exceeded = true; }
            else { node.visit_children_with(self); }
            self.depth -= 1;
        }
    )+};
}

impl Visit for Budget {
    // These node families cut every recursive path in the supported JS/TS AST.
    // JSX remains disabled; element/fragment roots are also handled explicitly.
    bounded_nodes!(visit_expr: Expr, visit_stmt: Stmt, visit_pat: Pat,
        visit_ts_type: TsType, visit_ts_entity_name: TsEntityName,
        visit_ts_namespace_body: TsNamespaceBody, visit_jsx_element: JSXElement,
        visit_jsx_fragment: JSXFragment);
}

enum Pending {
    Expr(Expr),
    Stmt(Stmt),
    Pat(Pat),
    Type(TsType),
    Name(TsEntityName),
    Namespace(TsNamespaceBody),
    Element(JSXElement),
    Fragment(JSXFragment),
}

#[derive(Default)]
struct Drain {
    pending: Vec<Pending>,
}

impl VisitMut for Drain {
    fn visit_mut_expr(&mut self, node: &mut Expr) {
        self.pending.push(Pending::Expr(node.take()));
    }
    fn visit_mut_stmt(&mut self, node: &mut Stmt) {
        self.pending.push(Pending::Stmt(node.take()));
    }
    fn visit_mut_pat(&mut self, node: &mut Pat) {
        self.pending.push(Pending::Pat(node.take()));
    }
    fn visit_mut_ts_type(&mut self, node: &mut TsType) {
        let empty = TsType::TsKeywordType(TsKeywordType {
            span: DUMMY_SP,
            kind: TsKeywordTypeKind::TsUnknownKeyword,
        });
        self.pending
            .push(Pending::Type(std::mem::replace(node, empty)));
    }
    fn visit_mut_ts_entity_name(&mut self, node: &mut TsEntityName) {
        let empty = TsEntityName::Ident(Ident::new_no_ctxt("".into(), DUMMY_SP));
        self.pending
            .push(Pending::Name(std::mem::replace(node, empty)));
    }
    fn visit_mut_ts_namespace_body(&mut self, node: &mut TsNamespaceBody) {
        let empty = TsNamespaceBody::TsModuleBlock(TsModuleBlock {
            span: DUMMY_SP,
            body: Vec::new(),
        });
        self.pending
            .push(Pending::Namespace(std::mem::replace(node, empty)));
    }
    fn visit_mut_jsx_element(&mut self, node: &mut JSXElement) {
        self.pending.push(Pending::Element(node.take()));
    }
    fn visit_mut_jsx_fragment(&mut self, node: &mut JSXFragment) {
        self.pending.push(Pending::Fragment(node.take()));
    }
}

pub(super) fn drop_module(mut module: Module) {
    // Taking recursive roots leaves only shallow placeholders in each old node.
    // No native-stack recursion occurs as the worklist or old nodes are dropped.
    let mut drain = Drain::default();
    module.visit_mut_children_with(&mut drain);
    while let Some(pending) = drain.pending.pop() {
        match pending {
            Pending::Expr(mut node) => node.visit_mut_children_with(&mut drain),
            Pending::Stmt(mut node) => node.visit_mut_children_with(&mut drain),
            Pending::Pat(mut node) => node.visit_mut_children_with(&mut drain),
            Pending::Type(mut node) => node.visit_mut_children_with(&mut drain),
            Pending::Name(mut node) => node.visit_mut_children_with(&mut drain),
            Pending::Namespace(mut node) => node.visit_mut_children_with(&mut drain),
            Pending::Element(mut node) => node.visit_mut_children_with(&mut drain),
            Pending::Fragment(mut node) => node.visit_mut_children_with(&mut drain),
        }
    }
}

#[cfg(test)]
mod tests {
    use swc_ecma_ast::{BinExpr, BinaryOp, ExprStmt, ModuleItem};

    use super::*;

    #[test]
    fn rejected_flat_ast_is_disposed_without_recursive_drop() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let mut expression = Expr::Ident(Ident::new_no_ctxt("value".into(), DUMMY_SP));
                for _ in 0..100_000 {
                    expression = Expr::Bin(BinExpr {
                        span: DUMMY_SP,
                        op: BinaryOp::Add,
                        left: Box::new(expression),
                        right: Box::new(Expr::Ident(Ident::new_no_ctxt("value".into(), DUMMY_SP))),
                    });
                }
                let module = Module {
                    span: DUMMY_SP,
                    body: vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: Box::new(expression),
                    }))],
                    shebang: None,
                };
                // Validate on a compiler stack, then explicitly drain on the small
                // caller stack. This protects the error path, not just successful emit.
                assert!(super::super::with_compiler_stack(|| validate(&module)).is_err());
                drop_module(module);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
