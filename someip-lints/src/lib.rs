#![feature(rustc_private)]
#![warn(unused_extern_crates)]

//! # SOME/IP Runtime Custom Lints
//!
//! This crate provides custom lints for the `someip-runtime` library using
//! [dylint](https://github.com/trailofbits/dylint), a tool for running Rust
//! lints from dynamic libraries.
//!
//! ## Available Lints
//!
//! - `runtime_shutdown_required`: Warns when `Runtime` is used without calling `shutdown()`

extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

use rustc_hir::def_id::LocalDefId;
use rustc_hir::intravisit::{walk_body, walk_expr, Visitor};
use rustc_hir::{Expr, ExprKind, HirId, LetStmt, PatKind, QPath};
use rustc_lint::{LateContext, LateLintPass, LintContext};
use rustc_middle::ty::{Ty, TyKind};
use rustc_session::declare_lint_pass;
use rustc_span::Span;

dylint_linting::dylint_library!();

/// Register all lints provided by this crate
#[doc(hidden)]
#[no_mangle]
pub fn register_lints(sess: &rustc_session::Session, lint_store: &mut rustc_lint::LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[RUNTIME_SHUTDOWN_REQUIRED]);
    lint_store.register_late_pass(|_| Box::new(RuntimeShutdownRequired));
}

// ============================================================================
// RUNTIME_SHUTDOWN_REQUIRED LINT
// ============================================================================

rustc_session::declare_lint! {
    /// ### What it does
    ///
    /// Checks for `someip_runtime::Runtime` variables that are not explicitly
    /// shut down by calling `.shutdown().await`.
    ///
    /// ### Why is this bad?
    ///
    /// Dropping a `Runtime` without calling `shutdown()` may cause pending RPC
    /// responses to be lost. The runtime task may be terminated before outgoing
    /// messages have been sent.
    ///
    /// ### Known limitations
    ///
    /// This lint uses heuristic analysis and may produce false positives:
    /// - It cannot track ownership through function calls
    /// - It may not detect shutdown() called in conditional branches
    /// - It doesn't track runtime passed to other functions
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// async fn bad_example() {
    ///     let runtime = Runtime::new(config).await?;
    ///     let proxy = runtime.find::<MyService>().await?;
    ///     // BAD: runtime dropped without shutdown()
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// async fn good_example() {
    ///     let runtime = Runtime::new(config).await?;
    ///     let proxy = runtime.find::<MyService>().await?;
    ///     runtime.shutdown().await;  // GOOD: explicit shutdown
    /// }
    /// ```
    pub RUNTIME_SHUTDOWN_REQUIRED,
    Warn,
    "someip_runtime::Runtime should be explicitly shut down with .shutdown().await"
}

declare_lint_pass!(RuntimeShutdownRequired => [RUNTIME_SHUTDOWN_REQUIRED]);

impl<'tcx> LateLintPass<'tcx> for RuntimeShutdownRequired {
    fn check_fn(
        &mut self,
        cx: &LateContext<'tcx>,
        _kind: rustc_hir::intravisit::FnKind<'tcx>,
        _decl: &'tcx rustc_hir::FnDecl<'tcx>,
        body: &'tcx rustc_hir::Body<'tcx>,
        _span: Span,
        _def_id: LocalDefId,
    ) {
        let mut visitor = RuntimeUsageVisitor::new(cx);
        walk_body(&mut visitor, body);
        visitor.report_missing_shutdowns();
    }
}

/// Visitor that tracks Runtime variable bindings and shutdown calls
struct RuntimeUsageVisitor<'a, 'tcx> {
    cx: &'a LateContext<'tcx>,
    /// Tracks (variable name, binding span, hir_id, has_shutdown_called)
    runtime_bindings: Vec<(String, Span, HirId, bool)>,
}

impl<'a, 'tcx> RuntimeUsageVisitor<'a, 'tcx> {
    fn new(cx: &'a LateContext<'tcx>) -> Self {
        Self {
            cx,
            runtime_bindings: Vec::new(),
        }
    }

    /// Check if a type is the someip_runtime::Runtime type (not RuntimeConfig, etc.)
    fn is_runtime_type(&self, ty: Ty<'tcx>) -> bool {
        if let TyKind::Adt(adt_def, _) = ty.kind() {
            let type_name = self.cx.tcx.def_path_str(adt_def.did());
            // Must be exactly "Runtime" (with possible generic args), not RuntimeConfig, etc.
            // The type path will be something like "someip_runtime::runtime::Runtime"
            // or just "Runtime<...>" in the crate itself
            let is_exact_runtime = type_name.ends_with("::Runtime")
                || type_name == "Runtime"
                || type_name.ends_with("::Runtime<_, _, _>")
                || type_name.starts_with("Runtime<");

            // Exclude RuntimeConfig, RuntimeBuilder, etc.
            let is_excluded = type_name.contains("RuntimeConfig")
                || type_name.contains("RuntimeBuilder")
                || type_name.contains("RuntimeInner");

            is_exact_runtime && !is_excluded
        } else {
            false
        }
    }

    /// Record a new runtime binding
    fn record_binding(&mut self, name: String, span: Span, hir_id: HirId) {
        self.runtime_bindings.push((name, span, hir_id, false));
    }

    /// Mark a runtime variable as having shutdown called
    fn mark_shutdown_called(&mut self, var_name: &str) {
        for (name, _, _, has_shutdown) in &mut self.runtime_bindings {
            if name == var_name {
                *has_shutdown = true;
            }
        }
    }

    /// Report warnings for runtime variables without shutdown
    fn report_missing_shutdowns(&self) {
        for (name, span, _, has_shutdown) in &self.runtime_bindings {
            if !has_shutdown {
                self.cx.span_lint(
                    RUNTIME_SHUTDOWN_REQUIRED,
                    *span,
                    |diag| {
                        diag.primary_message(format!(
                            "Runtime `{}` is dropped without calling `.shutdown().await`",
                            name
                        ));
                        diag.help(
                            "call `runtime.shutdown().await` before dropping to ensure pending \
                             RPC responses are sent. Alternatively, enable the `strict-shutdown` \
                             feature to panic on drop without shutdown."
                        );
                    }
                );
            }
        }
    }
}

impl<'a, 'tcx> Visitor<'tcx> for RuntimeUsageVisitor<'a, 'tcx> {
    type NestedFilter = rustc_middle::hir::nested_filter::OnlyBodies;

    fn maybe_tcx(&mut self) -> Self::MaybeTyCtxt {
        self.cx.tcx
    }

    fn visit_local(&mut self, local: &'tcx LetStmt<'tcx>) {
        // Check if this is a let binding of a Runtime type
        if let Some(init) = local.init {
            let ty = self.cx.typeck_results().expr_ty(init);
            if self.is_runtime_type(ty) {
                if let PatKind::Binding(_, hir_id, ident, _) = local.pat.kind {
                    self.record_binding(ident.name.to_string(), local.span, hir_id);
                }
            }
        }
    }

    fn visit_expr(&mut self, expr: &'tcx Expr<'tcx>) {
        // Check for method calls like `runtime.shutdown()`
        if let ExprKind::MethodCall(path_segment, receiver, _args, _span) = &expr.kind {
            if path_segment.ident.name.as_str() == "shutdown" {
                // Try to get the variable name from the receiver
                if let Some(var_name) = self.get_receiver_var_name(receiver) {
                    self.mark_shutdown_called(&var_name);
                }
            }
        }

        // Also check for `Runtime::shutdown(runtime)` style calls
        if let ExprKind::Call(func, args) = &expr.kind {
            if let ExprKind::Path(QPath::TypeRelative(_, segment)) = &func.kind {
                if segment.ident.name.as_str() == "shutdown" && !args.is_empty() {
                    if let Some(var_name) = self.get_receiver_var_name(&args[0]) {
                        self.mark_shutdown_called(&var_name);
                    }
                }
            }
        }

        walk_expr(self, expr);
    }
}

impl<'a, 'tcx> RuntimeUsageVisitor<'a, 'tcx> {
    /// Extract variable name from an expression (if it's a simple variable reference)
    fn get_receiver_var_name(&self, expr: &Expr<'tcx>) -> Option<String> {
        match &expr.kind {
            ExprKind::Path(QPath::Resolved(_, path)) => {
                path.segments.last().map(|seg| seg.ident.name.to_string())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_tests() {
        dylint_testing::ui_test(
            env!("CARGO_PKG_NAME"),
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui"),
        );
    }
}
