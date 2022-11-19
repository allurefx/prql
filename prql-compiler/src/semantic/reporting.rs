use std::ops::Range;

use anyhow::{Ok, Result};
use ariadne::{Color, Label, Report, ReportBuilder, ReportKind, Source};

use super::context::DeclKind;
use super::module::NS_DEFAULT_DB;
use super::{Context, Frame};
use crate::ast::ast_fold::*;
use crate::ast::*;
use crate::error::Span;

pub fn label_references(
    stmts: Vec<Stmt>,
    context: &Context,
    source_id: String,
    source: String,
) -> (Vec<u8>, Vec<Stmt>) {
    let mut report = Report::build(ReportKind::Custom("Info", Color::Blue), &source_id, 0);

    let source = Source::from(source);

    // label all idents and function calls
    let mut labeler = Labeler {
        context,
        source: &source,
        source_id: &source_id,
        report: &mut report,
    };
    labeler.fold_table_exprs();
    let stmts = labeler.fold_stmts(stmts).unwrap();

    let mut out = Vec::new();
    report
        .finish()
        .write((source_id, source), &mut out)
        .unwrap();
    (out, stmts)
}

/// Traverses AST and add labels for each of the idents and function calls
struct Labeler<'a> {
    context: &'a Context,
    source: &'a Source,
    source_id: &'a str,
    report: &'a mut ReportBuilder<(String, Range<usize>)>,
}

impl<'a> Labeler<'a> {
    fn fold_table_exprs(&mut self) {
        if let Some(default_db) = self.context.root_mod.names.get(NS_DEFAULT_DB) {
            let default_db = default_db.clone().kind.into_module().unwrap();

            for (_, decl) in default_db.names.into_iter() {
                if let DeclKind::TableDef {
                    expr: Some(expr), ..
                } = decl.kind
                {
                    self.fold_expr(*expr).unwrap();
                }
            }
        }
    }

    fn get_span_lines(&mut self, id: usize) -> Option<String> {
        let decl_span = self.context.span_map.get(&id);
        decl_span.map(|decl_span| {
            let line_span = self.source.get_line_range(&Range::from(*decl_span));
            if line_span.len() <= 1 {
                format!(" at line {}", line_span.start + 1)
            } else {
                format!(" at lines {}-{}", line_span.start + 1, line_span.end)
            }
        })
    }
}

impl<'a> AstFold for Labeler<'a> {
    fn fold_expr(&mut self, node: Expr) -> Result<Expr> {
        if let Some(ident) = node.kind.as_ident() {
            if let Some(span) = node.span {
                let decl = self.context.root_mod.get(ident);

                let ident = format!("[{ident}]");

                let (decl, color) = if let Some(decl) = decl {
                    let color = match &decl.kind {
                        DeclKind::Expr(_) => Color::Blue,
                        DeclKind::Column { .. } => Color::Yellow,
                        DeclKind::TableDef { .. } => Color::Red,
                        DeclKind::FuncDef(_) => Color::Magenta,
                        DeclKind::Module(_) => Color::Cyan,
                        DeclKind::LayeredModules(_) => Color::Cyan,
                        DeclKind::NoResolve => Color::White,
                        DeclKind::Wildcard(_) => Color::White,
                    };

                    let location = decl
                        .declared_at
                        .and_then(|id| self.get_span_lines(id))
                        .unwrap_or_default();

                    let decl = match &decl.kind {
                        DeclKind::TableDef { frame, .. } => format!("table {frame}"),
                        _ => decl.to_string(),
                    };

                    (format!("{decl}{location}"), color)
                } else if let Some(decl_id) = node.target_id {
                    let lines = self.get_span_lines(decl_id).unwrap_or_default();

                    (format!("variable{lines}"), Color::Yellow)
                } else {
                    ("".to_string(), Color::White)
                };

                self.report.add_label(
                    Label::new((self.source_id.to_string(), Range::from(span)))
                        .with_message(format!("{ident} {decl}"))
                        .with_color(color),
                );
            }
        }
        Ok(Expr {
            kind: self.fold_expr_kind(node.kind)?,
            ..node
        })
    }
}

pub fn collect_frames(stmts: Vec<Stmt>) -> Vec<(Span, Frame)> {
    let mut collector = FrameCollector { frames: vec![] };

    collector.fold_stmts(stmts).unwrap();

    collector.frames
}

/// Traverses AST and collects all node.frame
struct FrameCollector {
    frames: Vec<(Span, Frame)>,
}

impl AstFold for FrameCollector {
    fn fold_expr(&mut self, expr: Expr) -> Result<Expr> {
        if let ExprKind::TransformCall(tc) = &expr.kind {
            let span = match tc.kind.as_ref() {
                TransformKind::From(expr) => expr.span.unwrap(),
                TransformKind::Derive { tbl, .. }
                | TransformKind::Select { tbl, .. }
                | TransformKind::Filter { tbl, .. }
                | TransformKind::Aggregate { tbl, .. }
                | TransformKind::Sort { tbl, .. }
                | TransformKind::Take { tbl, .. }
                | TransformKind::Join { tbl, .. }
                | TransformKind::Group { tbl, .. }
                | TransformKind::Window { tbl, .. } => tbl.span.unwrap(),
            };

            let frame = expr.ty.clone().and_then(|t| t.into_table().ok());
            if let Some(frame) = frame {
                self.frames.push((span, frame));
            }
        }

        let mut expr = expr;
        expr.kind = self.fold_expr_kind(expr.kind)?;
        Ok(expr)
    }
}
