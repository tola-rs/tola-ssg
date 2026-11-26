//! Control-flow graph lowering for [`LiveSource`].

use typst_syntax::ast::{self, AstNode};
use typst_syntax::{LinkedNode, SyntaxKind};

use super::scan::{assignment_reads, written_ident};
use super::{Action, LiveSource};

impl<'a> LiveSource<'a> {
    fn flush_read(&mut self, region: usize, node: &LinkedNode, at: usize) {
        if !matches!(node.kind(), SyntaxKind::Ident | SyntaxKind::MathIdent) {
            return;
        }
        let range = node.range();
        if let Some(decls) = self.reads.remove(&(range.start, range.end)) {
            self.push_uses(region, at, &decls);
        }
    }

    fn flush_captures(&mut self, region: usize, creation: usize, at: usize) {
        if let Some(decls) = self.captures.remove(&creation) {
            self.push_uses(region, at, &decls);
        }
    }

    pub(super) fn push_uses(&mut self, region: usize, at: usize, decls: &[usize]) {
        let actions = &mut self.regions[region].nodes[at].actions;
        for &decl in decls {
            actions.push(Action::Use(decl));
        }
    }

    /// Builds one region's flow from its body, the whole source for region zero.
    pub(super) fn lower_region(&mut self, region: usize, body: &LinkedNode) {
        let entry = self.new_node(region);
        let exit = self.new_node(region);
        self.regions[region].entry = entry;
        self.regions[region].exit = exit;
        let saved = self.in_header;
        self.in_header = false;
        let tail = self.walk(body, region, entry);
        self.in_header = saved;
        if let Some(tail) = tail {
            self.edge(region, tail, exit);
        }
    }

    fn enter_region(&mut self, creation: usize, body: &LinkedNode, creation_node: usize) {
        let Some(&region) = self.creation_regions.get(&creation) else {
            return;
        };
        self.regions[region].creation_node = Some(creation_node);
        self.lower_region(region, body);
    }

    fn lower_loop_body(
        &mut self,
        region: usize,
        head: usize,
        exit: usize,
        entry: usize,
        body: Option<LinkedNode<'_>>,
    ) {
        self.regions[region].loops.push((exit, head));
        let saved = self.in_header;
        self.in_header = false;
        let lowered = body.and_then(|body| self.walk(&body, region, entry));
        self.in_header = saved;
        self.regions[region].loops.pop();
        if let Some(tail) = lowered {
            self.edge(region, tail, head);
        }
    }

    fn walk(&mut self, node: &LinkedNode, region: usize, cur: usize) -> Option<usize> {
        self.tick();
        if self.halt {
            return Some(cur);
        }
        match node.kind() {
            SyntaxKind::Ident | SyntaxKind::MathIdent => {
                self.flush_read(region, node, cur);
                return Some(cur);
            }
            SyntaxKind::CodeBlock | SyntaxKind::ContentBlock => {
                if let Some(body) = block_body(node) {
                    return self.sequence(&body, region, cur);
                }
            }
            SyntaxKind::Markup => return self.sequence(node, region, cur),
            SyntaxKind::Conditional => {
                if let Some(cond) = node.cast::<ast::Conditional>() {
                    return self.walk_conditional(node, cond, region, cur);
                }
            }
            SyntaxKind::ForLoop => {
                if let Some(for_loop) = node.cast::<ast::ForLoop>() {
                    return self.walk_for_loop(node, for_loop, region, cur);
                }
            }
            SyntaxKind::WhileLoop => {
                if let Some(while_loop) = node.cast::<ast::WhileLoop>() {
                    return self.walk_while_loop(node, while_loop, region, cur);
                }
            }
            SyntaxKind::LoopBreak | SyntaxKind::LoopContinue => {
                if self.in_header {
                    self.uncertain = true;
                }
                let target =
                    self.regions[region]
                        .loops
                        .last()
                        .map(|&(break_target, continue_target)| {
                            if node.kind() == SyntaxKind::LoopBreak {
                                break_target
                            } else {
                                continue_target
                            }
                        });
                if let Some(target) = target {
                    self.edge(region, cur, target);
                }
                return None;
            }
            SyntaxKind::FuncReturn => {
                if let Some(ret) = node.cast::<ast::FuncReturn>() {
                    let mut cur = cur;
                    if let Some(val) = ret.body().and_then(|body| node.find(body.span())) {
                        cur = self.walk(&val, region, cur)?;
                    }
                    let exit = self.regions[region].exit;
                    self.edge(region, cur, exit);
                }
                return None;
            }
            SyntaxKind::Closure => {
                if let Some(closure) = node.cast::<ast::Closure>() {
                    return self.walk_closure(node, closure, region, cur);
                }
            }
            SyntaxKind::Contextual => {
                if let Some(ctx) = node.cast::<ast::Contextual>() {
                    self.flush_captures(region, node.range().start, cur);
                    if let Some(body) = node.find(ctx.body().span()) {
                        self.enter_region(node.range().start, &body, cur);
                    }
                    return Some(cur);
                }
            }
            SyntaxKind::LetBinding => {
                if let Some(binding) = node.cast::<ast::LetBinding>() {
                    let mut cur = cur;
                    if let Some(init) = binding.init().and_then(|init| node.find(init.span())) {
                        cur = self.walk(&init, region, cur)?;
                    }
                    self.store_bindings(
                        region,
                        cur,
                        binding.kind().bindings(),
                        binding.init().is_some(),
                    );
                    return Some(cur);
                }
            }
            SyntaxKind::DestructAssignment => {
                if let Some(assign) = node.cast::<ast::DestructAssignment>() {
                    let mut cur = cur;
                    if let Some(val) = node.find(assign.value().span()) {
                        cur = self.walk(&val, region, cur)?;
                    }
                    // Compiler assigns targets left-to-right, so later indices observe earlier stores.
                    if let Some(pattern) = node.find(assign.pattern().span()) {
                        cur = self.lower_destructured(region, &pattern, assign.pattern(), cur)?;
                    }
                    return Some(cur);
                }
            }
            SyntaxKind::Binary => {
                if let Some(bin) = node.cast::<ast::Binary>() {
                    return self.walk_binary(node, bin, region, cur);
                }
            }
            _ => {}
        }
        let mut cur = cur;
        for child in node.children() {
            cur = self.walk(&child, region, cur)?;
        }
        Some(cur)
    }

    fn walk_conditional(
        &mut self,
        node: &LinkedNode,
        cond: ast::Conditional<'_>,
        region: usize,
        mut cur: usize,
    ) -> Option<usize> {
        if let Some(test) = node.find(cond.condition().span()) {
            cur = self.walk(&test, region, cur)?;
        }
        let join = self.new_node(region);
        if let Some(if_body) = node.find(cond.if_body().span()) {
            let entry = self.new_node(region);
            self.edge(region, cur, entry);
            if let Some(tail) = self.walk(&if_body, region, entry) {
                self.edge(region, tail, join);
            }
        }
        if let Some(else_body) = cond.else_body().and_then(|body| node.find(body.span())) {
            let entry = self.new_node(region);
            self.edge(region, cur, entry);
            if let Some(tail) = self.walk(&else_body, region, entry) {
                self.edge(region, tail, join);
            }
        } else {
            self.edge(region, cur, join);
        }
        Some(join)
    }

    fn walk_for_loop(
        &mut self,
        node: &LinkedNode,
        for_loop: ast::ForLoop<'_>,
        region: usize,
        mut cur: usize,
    ) -> Option<usize> {
        // Header break/continue targets an outer or unmodelled loop; mark uncertain.
        let saved = self.in_header;
        self.in_header = true;
        let header = match node.find(for_loop.iterable().span()) {
            Some(iterable) => self.walk(&iterable, region, cur),
            None => Some(cur),
        };
        self.in_header = saved;
        cur = header?;
        let head = self.new_node(region);
        self.edge(region, cur, head);
        let exit = self.new_node(region);
        let body_entry = self.new_node(region);
        self.edge(region, head, body_entry);
        self.edge(region, head, exit);
        // Pattern binding runs per iteration, never on the zero-iteration exit.
        self.store_bindings(region, body_entry, for_loop.pattern().bindings(), false);
        let body = node.find(for_loop.body().span());
        self.lower_loop_body(region, head, exit, body_entry, body);
        Some(exit)
    }

    fn walk_while_loop(
        &mut self,
        node: &LinkedNode,
        while_loop: ast::WhileLoop<'_>,
        region: usize,
        cur: usize,
    ) -> Option<usize> {
        let head = self.new_node(region);
        self.edge(region, cur, head);
        let saved = self.in_header;
        self.in_header = true;
        let cond = match node.find(while_loop.condition().span()) {
            Some(cond) => self.walk(&cond, region, head),
            None => Some(head),
        };
        self.in_header = saved;
        let exit = self.new_node(region);
        let body_entry = self.new_node(region);
        match cond {
            Some(tail) => {
                self.edge(region, tail, body_entry);
                self.edge(region, tail, exit);
            }
            None => self.edge(region, head, exit),
        }
        let body = node.find(while_loop.body().span());
        self.lower_loop_body(region, head, exit, body_entry, body);
        Some(exit)
    }

    fn walk_closure(
        &mut self,
        node: &LinkedNode,
        closure: ast::Closure<'_>,
        region: usize,
        mut cur: usize,
    ) -> Option<usize> {
        for param in closure.params().children() {
            let ast::Param::Named(named) = param else {
                continue;
            };
            let Some(default) = node.find(named.expr().span()) else {
                continue;
            };
            cur = self.walk(&default, region, cur)?;
        }
        if let Some(body) = node.find(closure.body().span()) {
            self.flush_captures(region, node.range().start, cur);
            self.enter_region(node.range().start, &body, cur);
        }
        Some(cur)
    }

    fn walk_binary(
        &mut self,
        node: &LinkedNode,
        bin: ast::Binary<'_>,
        region: usize,
        mut cur: usize,
    ) -> Option<usize> {
        match bin.op() {
            ast::BinOp::And | ast::BinOp::Or => {
                if let Some(lhs) = node.find(bin.lhs().span()) {
                    cur = self.walk(&lhs, region, cur)?;
                }
                let join = self.new_node(region);
                self.edge(region, cur, join);
                let rhs_entry = self.new_node(region);
                self.edge(region, cur, rhs_entry);
                if let Some(rhs) = node.find(bin.rhs().span())
                    && let Some(tail) = self.walk(&rhs, region, rhs_entry)
                {
                    self.edge(region, tail, join);
                }
                Some(join)
            }
            op => {
                // Assignments evaluate rhs before accessing lhs (typst-eval `apply_assignment`).
                let stores = assignment_reads(op).is_some();
                let order = if stores {
                    [bin.rhs(), bin.lhs()]
                } else {
                    [bin.lhs(), bin.rhs()]
                };
                for side in order {
                    if let Some(side) = node.find(side.span()) {
                        cur = self.walk(&side, region, cur)?;
                    }
                }
                if stores && let Some(ident) = written_ident(bin.lhs()) {
                    self.store_written(region, cur, ident);
                }
                Some(cur)
            }
        }
    }

    fn store_bindings(
        &mut self,
        region: usize,
        node: usize,
        bindings: Vec<ast::Ident<'_>>,
        report: bool,
    ) {
        for name in bindings {
            let Some(range) = self.range_of(name.span()) else {
                continue;
            };
            let Some(decl) = self.names.declared_at(range.start) else {
                continue;
            };
            let store_range = self.names.declarations()[decl].range.clone();
            self.regions[region].nodes[node]
                .actions
                .push(Action::Store {
                    declaration: decl,
                    range: store_range,
                    report,
                });
            self.init_nodes[decl] = Some((region, node));
        }
    }

    fn store_written(&mut self, region: usize, node: usize, ident: ast::Ident<'_>) {
        let Some(range) = self.range_of(ident.span()) else {
            return;
        };
        let Some(site) = self.writes.get(&range.start) else {
            return;
        };
        let (Some(decl), true) = (site.declaration, site.real) else {
            return;
        };
        let range = site.range.clone();
        self.regions[region].nodes[node]
            .actions
            .push(Action::Store {
                declaration: decl,
                range,
                report: true,
            });
    }

    fn lower_destructured(
        &mut self,
        region: usize,
        pattern_node: &LinkedNode,
        pattern: ast::Pattern<'_>,
        cur: usize,
    ) -> Option<usize> {
        match pattern {
            ast::Pattern::Destructuring(destructuring) => {
                let mut cur = cur;
                for item in destructuring.items() {
                    let Some(item_node) = pattern_node.find(item.span()) else {
                        continue;
                    };
                    cur = self.lower_destructuring_item(region, &item_node, item, cur)?;
                }
                Some(cur)
            }
            ast::Pattern::Parenthesized(group) => match pattern_node.find(group.pattern().span()) {
                Some(inner) => self.lower_destructured(region, &inner, group.pattern(), cur),
                None => Some(cur),
            },
            ast::Pattern::Normal(expr) => self.lower_target(region, pattern_node, expr, cur),
            ast::Pattern::Placeholder(_) => Some(cur),
        }
    }

    fn lower_target(
        &mut self,
        region: usize,
        target: &LinkedNode,
        expr: ast::Expr<'_>,
        cur: usize,
    ) -> Option<usize> {
        let cur = self.walk(target, region, cur)?;
        if let ast::Expr::Ident(ident) = expr {
            self.store_written(region, cur, ident);
        }
        Some(cur)
    }

    fn lower_destructuring_item(
        &mut self,
        region: usize,
        item_node: &LinkedNode,
        item: ast::DestructuringItem<'_>,
        cur: usize,
    ) -> Option<usize> {
        match item {
            ast::DestructuringItem::Pattern(pattern) => {
                self.lower_destructured(region, item_node, pattern, cur)
            }
            ast::DestructuringItem::Named(named) => match item_node.find(named.pattern().span()) {
                Some(target) => self.lower_destructured(region, &target, named.pattern(), cur),
                None => Some(cur),
            },
            ast::DestructuringItem::Spread(spread) => {
                let Some(expr) = spread.sink_expr() else {
                    return Some(cur);
                };
                let Some(target) = item_node.find(expr.span()) else {
                    return Some(cur);
                };
                self.lower_target(region, &target, expr, cur)
            }
        }
    }

    fn sequence(&mut self, block: &LinkedNode, region: usize, entry: usize) -> Option<usize> {
        let mut cur = Some(entry);
        for child in block.children() {
            if self.halt {
                break;
            }
            if child.kind().is_trivia() {
                continue;
            }
            let Some(node) = cur else {
                break;
            };
            cur = self.walk(&child, region, node);
        }
        cur
    }
}

fn block_body<'a>(node: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    let span = if let Some(block) = node.cast::<ast::CodeBlock>() {
        block.body().span()
    } else {
        node.cast::<ast::ContentBlock>()?.body().span()
    };
    node.children().find(|child| child.span() == span)
}
