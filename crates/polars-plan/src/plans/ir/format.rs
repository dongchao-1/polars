use std::fmt::{self, Display, Formatter};

use polars_core::schema::Schema;
use polars_io::RowIndex;
use polars_utils::format_list_truncated;
use recursive::recursive;

use self::ir::dot::ScanSourcesDisplay;
use crate::prelude::*;

pub struct IRDisplay<'a> {
    is_streaming: bool,
    lp: IRPlanRef<'a>,
}

#[derive(Clone, Copy)]
pub struct ExprIRDisplay<'a> {
    pub(crate) node: Node,
    pub(crate) output_name: &'a OutputName,
    pub(crate) expr_arena: &'a Arena<AExpr>,
}

impl<'a> ExprIRDisplay<'a> {
    pub fn display_node(node: Node, expr_arena: &'a Arena<AExpr>) -> Self {
        Self {
            node,
            output_name: &OutputName::None,
            expr_arena,
        }
    }
}

/// Utility structure to display several [`ExprIR`]'s in a nice way
pub(crate) struct ExprIRSliceDisplay<'a, T: AsExpr> {
    pub(crate) exprs: &'a [T],
    pub(crate) expr_arena: &'a Arena<AExpr>,
}

pub(crate) trait AsExpr {
    fn node(&self) -> Node;
    fn output_name(&self) -> &OutputName;
}

impl AsExpr for Node {
    fn node(&self) -> Node {
        *self
    }
    fn output_name(&self) -> &OutputName {
        &OutputName::None
    }
}

impl AsExpr for ExprIR {
    fn node(&self) -> Node {
        self.node()
    }
    fn output_name(&self) -> &OutputName {
        self.output_name_inner()
    }
}

#[allow(clippy::too_many_arguments)]
fn write_scan(
    f: &mut Formatter,
    name: &str,
    sources: &ScanSources,
    indent: usize,
    n_columns: i64,
    total_columns: usize,
    predicate: &Option<ExprIRDisplay<'_>>,
    slice: Option<(i64, usize)>,
    row_index: Option<&RowIndex>,
) -> fmt::Result {
    write!(
        f,
        "{:indent$}{name} SCAN {}",
        "",
        ScanSourcesDisplay(sources)
    )?;

    let total_columns = total_columns - usize::from(row_index.is_some());
    if n_columns > 0 {
        write!(
            f,
            "\n{:indent$}PROJECT {n_columns}/{total_columns} COLUMNS",
            "",
        )?;
    } else {
        write!(f, "\n{:indent$}PROJECT */{total_columns} COLUMNS", "")?;
    }
    if let Some(predicate) = predicate {
        write!(f, "\n{:indent$}SELECTION: {predicate}", "")?;
    }
    if let Some(slice) = slice {
        write!(f, "\n{:indent$}SLICE: {slice:?}", "")?;
    }
    if let Some(row_index) = row_index {
        write!(f, "\n{:indent$}ROW_INDEX: {}", "", row_index.name)?;
        if row_index.offset != 0 {
            write!(f, " (offset: {})", row_index.offset)?;
        }
    }
    Ok(())
}

impl<'a> IRDisplay<'a> {
    pub fn new(lp: IRPlanRef<'a>) -> Self {
        if let Some(streaming_lp) = lp.extract_streaming_plan() {
            return Self::new_streaming(streaming_lp);
        }

        Self {
            is_streaming: false,
            lp,
        }
    }

    fn new_streaming(lp: IRPlanRef<'a>) -> Self {
        Self {
            is_streaming: true,
            lp,
        }
    }

    fn root(&self) -> &IR {
        self.lp.root()
    }

    fn with_root(&self, root: Node) -> Self {
        Self {
            is_streaming: false,
            lp: self.lp.with_root(root),
        }
    }

    fn display_expr(&self, root: &'a ExprIR) -> ExprIRDisplay<'a> {
        ExprIRDisplay {
            node: root.node(),
            output_name: root.output_name_inner(),
            expr_arena: self.lp.expr_arena,
        }
    }

    fn display_expr_slice(&self, exprs: &'a [ExprIR]) -> ExprIRSliceDisplay<'a, ExprIR> {
        ExprIRSliceDisplay {
            exprs,
            expr_arena: self.lp.expr_arena,
        }
    }

    #[recursive]
    fn _format(&self, f: &mut Formatter, indent: usize) -> fmt::Result {
        let indent_increment = 2;
        let indent = if self.is_streaming {
            writeln!(f, "{:indent$}STREAMING:", "")?;
            indent + indent_increment
        } else {
            if indent != 0 {
                writeln!(f)?;
            }
            indent
        };

        let sub_indent = indent + indent_increment;
        use IR::*;

        match self.root() {
            #[cfg(feature = "python")]
            PythonScan { options } => {
                let total_columns = options.schema.len();
                let n_columns = options
                    .with_columns
                    .as_ref()
                    .map(|s| s.len() as i64)
                    .unwrap_or(-1);

                let predicate = match &options.predicate {
                    PythonPredicate::Polars(e) => Some(self.display_expr(e)),
                    PythonPredicate::PyArrow(_) => None,
                    PythonPredicate::None => None,
                };

                write_scan(
                    f,
                    "PYTHON",
                    &ScanSources::default(),
                    indent,
                    n_columns,
                    total_columns,
                    &predicate,
                    options.n_rows.map(|x| (0, x)),
                    None,
                )
            },
            Union { inputs, options } => {
                let name = if let Some(slice) = options.slice {
                    format!("SLICED UNION: {slice:?}")
                } else {
                    "UNION".to_string()
                };

                // 3 levels of indentation
                // - 0 => UNION ... END UNION
                // - 1 => PLAN 0, PLAN 1, ... PLAN N
                // - 2 => actual formatting of plans
                let sub_sub_indent = sub_indent + indent_increment;
                write!(f, "{:indent$}{name}", "")?;
                for (i, plan) in inputs.iter().enumerate() {
                    write!(f, "\n{:sub_indent$}PLAN {i}:", "")?;
                    self.with_root(*plan)._format(f, sub_sub_indent)?;
                }
                write!(f, "\n{:indent$}END {name}", "")
            },
            HConcat { inputs, .. } => {
                let sub_sub_indent = sub_indent + indent_increment;
                write!(f, "{:indent$}HCONCAT", "")?;
                for (i, plan) in inputs.iter().enumerate() {
                    write!(f, "\n{:sub_indent$}PLAN {i}:", "")?;
                    self.with_root(*plan)._format(f, sub_sub_indent)?;
                }
                write!(f, "\n{:indent$}END HCONCAT", "")
            },
            Cache {
                input,
                id,
                cache_hits,
            } => {
                write!(
                    f,
                    "{:indent$}CACHE[id: {:x}, cache_hits: {}]",
                    "", *id, *cache_hits
                )?;
                self.with_root(*input)._format(f, sub_indent)
            },
            Scan {
                sources,
                file_info,
                predicate,
                scan_type,
                file_options,
                ..
            } => {
                let n_columns = file_options
                    .with_columns
                    .as_ref()
                    .map(|columns| columns.len() as i64)
                    .unwrap_or(-1);

                let predicate = predicate.as_ref().map(|p| self.display_expr(p));

                write_scan(
                    f,
                    (&**scan_type).into(),
                    sources,
                    indent,
                    n_columns,
                    file_info.schema.len(),
                    &predicate,
                    file_options.pre_slice,
                    file_options.row_index.as_ref(),
                )
            },
            Filter { predicate, input } => {
                let predicate = self.display_expr(predicate);
                // this one is writeln because we don't increase indent (which inserts a line)
                write!(f, "{:indent$}FILTER {predicate}", "")?;
                write!(f, "\n{:indent$}FROM", "")?;
                self.with_root(*input)._format(f, sub_indent)
            },
            DataFrameScan {
                schema,
                output_schema,
                ..
            } => {
                let total_columns = schema.len();
                let (n_columns, projected) = if let Some(schema) = output_schema {
                    (
                        format!("{}", schema.len()),
                        format_list_truncated!(schema.iter_names(), 4, '"'),
                    )
                } else {
                    ("*".to_string(), "".to_string())
                };
                write!(
                    f,
                    "{:indent$}DF {}; PROJECT{} {}/{} COLUMNS",
                    "",
                    format_list_truncated!(schema.iter_names(), 4, '"'),
                    projected,
                    n_columns,
                    total_columns,
                )
            },
            Select { expr, input, .. } => {
                // @NOTE: Maybe there should be a clear delimiter here?
                let exprs = self.display_expr_slice(expr);
                write!(f, "{:indent$}SELECT {exprs}", "")?;
                write!(f, "\n{:indent$}FROM", "")?;
                self.with_root(*input)._format(f, sub_indent)
            },
            Sort {
                input, by_column, ..
            } => {
                let by_column = self.display_expr_slice(by_column);
                write!(f, "{:indent$}SORT BY {by_column}", "")?;
                self.with_root(*input)._format(f, sub_indent)
            },
            GroupBy {
                input,
                keys,
                aggs,
                apply,
                ..
            } => {
                let keys = self.display_expr_slice(keys);
                write!(f, "{:indent$}AGGREGATE", "")?;
                if apply.is_some() {
                    write!(f, "\n{:sub_indent$}MAP_GROUPS BY {keys}", "")?;
                    write!(f, "\n{:sub_indent$}FROM", "")?;
                } else {
                    let aggs = self.display_expr_slice(aggs);
                    write!(f, "\n{:sub_indent$}{aggs} BY {keys}", "")?;
                    write!(f, "\n{:sub_indent$}FROM", "")?;
                }
                self.with_root(*input)._format(f, sub_indent)
            },
            Join {
                input_left,
                input_right,
                left_on,
                right_on,
                options,
                ..
            } => {
                let left_on = self.display_expr_slice(left_on);
                let right_on = self.display_expr_slice(right_on);

                // Fused cross + filter (show as nested loop join)
                if let Some(JoinTypeOptionsIR::Cross { predicate }) = &options.options {
                    let predicate = self.display_expr(predicate);
                    let name = "NESTED LOOP";
                    write!(f, "{:indent$}{name} JOIN ON {predicate}:", "")?;
                    write!(f, "\n{:indent$}LEFT PLAN:", "")?;
                    self.with_root(*input_left)._format(f, sub_indent)?;
                    write!(f, "\n{:indent$}RIGHT PLAN:", "")?;
                    self.with_root(*input_right)._format(f, sub_indent)?;
                    write!(f, "\n{:indent$}END {name} JOIN", "")
                } else {
                    let how = &options.args.how;
                    write!(f, "{:indent$}{how} JOIN:", "")?;
                    write!(f, "\n{:indent$}LEFT PLAN ON: {left_on}", "")?;
                    self.with_root(*input_left)._format(f, sub_indent)?;
                    write!(f, "\n{:indent$}RIGHT PLAN ON: {right_on}", "")?;
                    self.with_root(*input_right)._format(f, sub_indent)?;
                    write!(f, "\n{:indent$}END {how} JOIN", "")
                }
            },
            HStack { input, exprs, .. } => {
                // @NOTE: Maybe there should be a clear delimiter here?
                let exprs = self.display_expr_slice(exprs);

                write!(f, "{:indent$} WITH_COLUMNS:", "",)?;
                write!(f, "\n{:indent$} {exprs} ", "")?;
                self.with_root(*input)._format(f, sub_indent)
            },
            Distinct { input, options } => {
                write!(
                    f,
                    "{:indent$}UNIQUE[maintain_order: {:?}, keep_strategy: {:?}] BY {:?}",
                    "", options.maintain_order, options.keep_strategy, options.subset
                )?;
                self.with_root(*input)._format(f, sub_indent)
            },
            Slice { input, offset, len } => {
                write!(f, "{:indent$}SLICE[offset: {offset}, len: {len}]", "")?;
                self.with_root(*input)._format(f, sub_indent)
            },
            MapFunction {
                input, function, ..
            } => {
                if let Some(streaming_lp) = function.to_streaming_lp() {
                    IRDisplay::new_streaming(streaming_lp)._format(f, indent)
                } else {
                    write!(f, "{:indent$}{function}", "")?;
                    self.with_root(*input)._format(f, sub_indent)
                }
            },
            ExtContext { input, .. } => {
                write!(f, "{:indent$}EXTERNAL_CONTEXT", "")?;
                self.with_root(*input)._format(f, sub_indent)
            },
            Sink { input, payload, .. } => {
                let name = match payload {
                    SinkTypeIR::Memory => "SINK (memory)",
                    SinkTypeIR::File { .. } => "SINK (file)",
                    SinkTypeIR::Partition { .. } => "SINK (partition)",
                };
                write!(f, "{:indent$}{name}", "")?;
                self.with_root(*input)._format(f, sub_indent)
            },
            SinkMultiple { inputs } => {
                // 3 levels of indentation
                // - 0 => SINK_MULTIPLE ... END SINK_MULTIPLE
                // - 1 => PLAN 0, PLAN 1, ... PLAN N
                // - 2 => actual formatting of plans
                let sub_sub_indent = sub_indent + 2;
                write!(f, "{:indent$}SINK_MULTIPLE", "")?;
                for (i, plan) in inputs.iter().enumerate() {
                    write!(f, "\n{:sub_indent$}PLAN {i}:", "")?;
                    self.with_root(*plan)._format(f, sub_sub_indent)?;
                }
                write!(f, "\n{:indent$}END SINK_MULTIPLE", "")
            },
            SimpleProjection { input, columns } => {
                let num_columns = columns.as_ref().len();
                let total_columns = self.lp.lp_arena.get(*input).schema(self.lp.lp_arena).len();

                let columns = ColumnsDisplay(columns.as_ref());
                write!(
                    f,
                    "{:indent$}simple π {num_columns}/{total_columns} [{columns}]",
                    ""
                )?;

                self.with_root(*input)._format(f, sub_indent)
            },
            #[cfg(feature = "merge_sorted")]
            MergeSorted {
                input_left,
                input_right,
                key,
            } => {
                write!(f, "{:indent$}MERGE SORTED ON '{key}':", "")?;
                write!(f, "\n{:indent$}LEFT PLAN:", "")?;
                self.with_root(*input_left)._format(f, sub_indent)?;
                write!(f, "\n{:indent$}RIGHT PLAN:", "")?;
                self.with_root(*input_right)._format(f, sub_indent)?;
                write!(f, "\n{:indent$}END MERGE_SORTED", "")
            },
            Invalid => write!(f, "{:indent$}INVALID", ""),
        }
    }
}

impl<'a> ExprIRDisplay<'a> {
    fn with_slice<T: AsExpr>(&self, exprs: &'a [T]) -> ExprIRSliceDisplay<'a, T> {
        ExprIRSliceDisplay {
            exprs,
            expr_arena: self.expr_arena,
        }
    }

    fn with_root<T: AsExpr>(&self, root: &'a T) -> Self {
        Self {
            node: root.node(),
            output_name: root.output_name(),
            expr_arena: self.expr_arena,
        }
    }
}

impl Display for IRDisplay<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self._format(f, 0)
    }
}

impl fmt::Debug for IRDisplay<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self, f)
    }
}

impl<T: AsExpr> Display for ExprIRSliceDisplay<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // Display items in slice delimited by a comma

        use std::fmt::Write;

        let mut iter = self.exprs.iter();

        f.write_char('[')?;
        if let Some(fst) = iter.next() {
            let fst = ExprIRDisplay {
                node: fst.node(),
                output_name: fst.output_name(),
                expr_arena: self.expr_arena,
            };
            write!(f, "{fst}")?;
        }

        for expr in iter {
            let expr = ExprIRDisplay {
                node: expr.node(),
                output_name: expr.output_name(),
                expr_arena: self.expr_arena,
            };
            write!(f, ", {expr}")?;
        }

        f.write_char(']')?;

        Ok(())
    }
}

impl<T: AsExpr> fmt::Debug for ExprIRSliceDisplay<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl Display for ExprIRDisplay<'_> {
    #[recursive]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let root = self.expr_arena.get(self.node);

        use AExpr::*;
        match root {
            Window {
                function,
                partition_by,
                order_by,
                options,
            } => {
                let function = self.with_root(function);
                let partition_by = self.with_slice(partition_by);
                match options {
                    #[cfg(feature = "dynamic_group_by")]
                    WindowType::Rolling(options) => {
                        write!(
                            f,
                            "{function}.rolling(by='{}', offset={}, period={})",
                            options.index_column, options.offset, options.period
                        )
                    },
                    _ => {
                        if let Some((order_by, _)) = order_by {
                            let order_by = self.with_root(order_by);
                            write!(
                                f,
                                "{function}.over(partition_by: {partition_by}, order_by: {order_by})"
                            )
                        } else {
                            write!(f, "{function}.over({partition_by})")
                        }
                    },
                }
            },
            Len => write!(f, "len()"),
            Explode(expr) => {
                let expr = self.with_root(expr);
                write!(f, "{expr}.explode()")
            },
            Alias(expr, name) => {
                let expr = self.with_root(expr);
                write!(f, "{expr}.alias(\"{name}\")")
            },
            Column(name) => write!(f, "col(\"{name}\")"),
            Literal(v) => write!(f, "{v:?}"),
            BinaryExpr { left, op, right } => {
                let left = self.with_root(left);
                let right = self.with_root(right);
                write!(f, "[({left}) {op:?} ({right})]")
            },
            Sort { expr, options } => {
                let expr = self.with_root(expr);
                if options.descending {
                    write!(f, "{expr}.sort(desc)")
                } else {
                    write!(f, "{expr}.sort(asc)")
                }
            },
            SortBy {
                expr,
                by,
                sort_options,
            } => {
                let expr = self.with_root(expr);
                let by = self.with_slice(by);
                write!(f, "{expr}.sort_by(by={by}, sort_option={sort_options:?})",)
            },
            Filter { input, by } => {
                let input = self.with_root(input);
                let by = self.with_root(by);

                write!(f, "{input}.filter({by})")
            },
            Gather {
                expr,
                idx,
                returns_scalar,
            } => {
                let expr = self.with_root(expr);
                let idx = self.with_root(idx);
                expr.fmt(f)?;

                if *returns_scalar {
                    write!(f, ".get({idx})")
                } else {
                    write!(f, ".gather({idx})")
                }
            },
            Agg(agg) => {
                use IRAggExpr::*;
                match agg {
                    Min {
                        input,
                        propagate_nans,
                    } => {
                        self.with_root(input).fmt(f)?;
                        if *propagate_nans {
                            write!(f, ".nan_min()")
                        } else {
                            write!(f, ".min()")
                        }
                    },
                    Max {
                        input,
                        propagate_nans,
                    } => {
                        self.with_root(input).fmt(f)?;
                        if *propagate_nans {
                            write!(f, ".nan_max()")
                        } else {
                            write!(f, ".max()")
                        }
                    },
                    Median(expr) => write!(f, "{}.median()", self.with_root(expr)),
                    Mean(expr) => write!(f, "{}.mean()", self.with_root(expr)),
                    First(expr) => write!(f, "{}.first()", self.with_root(expr)),
                    Last(expr) => write!(f, "{}.last()", self.with_root(expr)),
                    Implode(expr) => write!(f, "{}.implode()", self.with_root(expr)),
                    NUnique(expr) => write!(f, "{}.n_unique()", self.with_root(expr)),
                    Sum(expr) => write!(f, "{}.sum()", self.with_root(expr)),
                    AggGroups(expr) => write!(f, "{}.groups()", self.with_root(expr)),
                    Count(expr, _) => write!(f, "{}.count()", self.with_root(expr)),
                    Var(expr, _) => write!(f, "{}.var()", self.with_root(expr)),
                    Std(expr, _) => write!(f, "{}.std()", self.with_root(expr)),
                    Quantile { expr, .. } => write!(f, "{}.quantile()", self.with_root(expr)),
                }
            },
            Cast {
                expr,
                dtype,
                options,
            } => {
                self.with_root(expr).fmt(f)?;
                if options.strict() {
                    write!(f, ".strict_cast({dtype:?})")
                } else {
                    write!(f, ".cast({dtype:?})")
                }
            },
            Ternary {
                predicate,
                truthy,
                falsy,
            } => {
                let predicate = self.with_root(predicate);
                let truthy = self.with_root(truthy);
                let falsy = self.with_root(falsy);
                write!(f, "when({predicate}).then({truthy}).otherwise({falsy})",)
            },
            Function {
                input, function, ..
            } => {
                let fst = self.with_root(&input[0]);
                fst.fmt(f)?;
                if input.len() >= 2 {
                    write!(f, ".{function}({})", self.with_slice(&input[1..]))
                } else {
                    write!(f, ".{function}()")
                }
            },
            AnonymousFunction { input, options, .. } => {
                let fst = self.with_root(&input[0]);
                fst.fmt(f)?;
                if input.len() >= 2 {
                    write!(f, ".{}({})", options.fmt_str, self.with_slice(&input[1..]))
                } else {
                    write!(f, ".{}()", options.fmt_str)
                }
            },
            Slice {
                input,
                offset,
                length,
            } => {
                let input = self.with_root(input);
                let offset = self.with_root(offset);
                let length = self.with_root(length);

                write!(f, "{input}.slice(offset={offset}, length={length})")
            },
        }?;

        match self.output_name {
            OutputName::None => {},
            OutputName::LiteralLhs(_) => {},
            OutputName::ColumnLhs(_) => {},
            #[cfg(feature = "dtype-struct")]
            OutputName::Field(_) => {},
            OutputName::Alias(name) => write!(f, r#".alias("{name}")"#)?,
        }

        Ok(())
    }
}

impl fmt::Debug for ExprIRDisplay<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

pub(crate) struct ColumnsDisplay<'a>(pub(crate) &'a Schema);

impl fmt::Display for ColumnsDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let len = self.0.len();
        let mut iter_names = self.0.iter_names().enumerate();

        const MAX_LEN: usize = 32;
        const ADD_PER_ITEM: usize = 4;

        let mut current_len = 0;

        if let Some((_, fst)) = iter_names.next() {
            write!(f, "\"{fst}\"")?;

            current_len += fst.len() + ADD_PER_ITEM;
        }

        for (i, col) in iter_names {
            current_len += col.len() + ADD_PER_ITEM;

            if current_len > MAX_LEN {
                write!(f, ", ... {} other ", len - i)?;
                if len - i == 1 {
                    f.write_str("column")?;
                } else {
                    f.write_str("columns")?;
                }

                break;
            }

            write!(f, ", \"{col}\"")?;
        }

        Ok(())
    }
}

impl fmt::Debug for Operator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl fmt::Debug for LiteralValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use LiteralValue::*;

        match self {
            Self::Scalar(sc) => write!(f, "{}", sc.value()),
            Self::Series(s) => {
                let name = s.name();
                if name.is_empty() {
                    write!(f, "Series")
                } else {
                    write!(f, "Series[{name}]")
                }
            },
            Range(range) => fmt::Debug::fmt(range, f),
            Dyn(d) => fmt::Debug::fmt(d, f),
        }
    }
}

impl fmt::Debug for DynLiteralValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => write!(f, "dyn int: {v}"),
            Self::Float(v) => write!(f, "dyn float: {}", v),
            Self::Str(v) => write!(f, "dyn str: {v}"),
            Self::List(_) => todo!(),
        }
    }
}

impl fmt::Debug for RangeLiteralValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "range({}, {})", self.low, self.high)
    }
}
