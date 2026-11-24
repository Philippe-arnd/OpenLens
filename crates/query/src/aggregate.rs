//! Aggregate-only SQL, for callers that must not see people (Ask without
//! person-level detail, D-021).
//!
//! The query runs in its own context, where `events` is a view with no
//! person-level column: no ids, URLs, properties or account, and no
//! `persons` or `person_aliases` table. People are counted through
//! `user_key` (and sessions through `session_key`), a keyed hash that only
//! lives for this project. The plan must end in an aggregate, has no
//! window function, and every group of fewer than [`MIN_USERS`] people is
//! dropped.

use std::sync::Arc;

use arrow::array::{Array, ArrayRef, StringArray, StringBuilder};
use arrow::datatypes::DataType;
use datafusion::common::tree_node::{Transformed, TreeNode, TreeNodeRecursion};
use datafusion::error::DataFusionError;
use datafusion::functions_aggregate::expr_fn::count_distinct;
use datafusion::logical_expr::expr_rewriter::normalize_col;
use datafusion::logical_expr::{
    Aggregate, ColumnarValue, Expr, Filter, LogicalPlan, Projection, ScalarUDF, Volatility, col,
    create_udf, lit,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Groups with fewer distinct people than this are left out of the result.
pub const MIN_USERS: i64 = 5;

/// Name of the hashing function, only registered where the view is built.
pub(crate) const PSEUDONYM_FN: &str = "openlens_pseudonym";

const USERS_COL: &str = "__openlens_users";

/// Columns of the aggregate view, besides `user_key` and `session_key`.
const VIEW_COLUMNS: &str =
    "event, timestamp, path, feature, role, plan, release, environment, flags, date";

/// `SELECT` that builds the aggregate view over `source` (`events`, or the
/// feature map's projection).
pub(crate) fn view_sql(source: &str) -> String {
    format!(
        "SELECT {VIEW_COLUMNS}, {PSEUDONYM_FN}(distinct_id) AS user_key, \
         {PSEUDONYM_FN}(session_id) AS session_key FROM {source}"
    )
}

/// Keyed hash of each string: 16 hex characters of HMAC-SHA256.
pub(crate) fn pseudonym_udf(key: Vec<u8>) -> ScalarUDF {
    create_udf(
        PSEUDONYM_FN,
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(move |args: &[ColumnarValue]| {
            let arg = args
                .first()
                .ok_or_else(|| DataFusionError::Plan("one argument expected".into()))?;
            let arr = match arg {
                ColumnarValue::Array(a) => a.clone(),
                ColumnarValue::Scalar(s) => s.to_array()?,
            };
            let arr = arrow::compute::cast(&arr, &DataType::Utf8)?;
            let strings = arr
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| DataFusionError::Plan("text expected".into()))?;
            let mut out = StringBuilder::with_capacity(strings.len(), strings.len() * 16);
            for v in strings.iter() {
                match v {
                    None => out.append_null(),
                    Some(v) => out.append_value(pseudonym(&key, v)),
                }
            }
            Ok(ColumnarValue::Array(Arc::new(out.finish()) as ArrayRef))
        }),
    )
}

fn pseudonym(key: &[u8], value: &str) -> String {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return String::new();
    };
    mac.update(value.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn has_subquery(e: &Expr) -> bool {
    e.exists(|x| {
        Ok(matches!(
            x,
            Expr::ScalarSubquery(_) | Expr::Exists(_) | Expr::InSubquery(_)
        ))
    })
    .unwrap_or(true)
}

fn is_pseudonym_column(e: &Expr) -> bool {
    matches!(e, Expr::Column(c) if c.name == "user_key" || c.name == "session_key")
}

/// Is `e` exactly `openlens_pseudonym(<source>)` aliased `alias`, the shape
/// [`view_sql`] gives `user_key` and `session_key`?
fn is_real_pseudonym(e: &Expr, alias: &str, source: &str) -> bool {
    let Expr::Alias(a) = e else { return false };
    if a.name != alias {
        return false;
    }
    matches!(
        a.expr.as_ref(),
        Expr::ScalarFunction(f)
            if f.func.name() == PSEUDONYM_FN
                && matches!(f.args.as_slice(), [Expr::Column(c)] if c.name == source)
    )
}

/// NSEC-01: a column named `user_key` or `session_key` can only reach the
/// aggregate by being the real pseudonym the view computed, never a value
/// an attacker renamed in a sub-query under `FROM`. Below the aggregate we
/// therefore accept any number of filters and sub-query aliases (the
/// `events` view is itself one or more nested views), but the first
/// projection we meet going down must be the view's own: it must define
/// `user_key` as `openlens_pseudonym(distinct_id)` and `session_key` as
/// `openlens_pseudonym(session_id)`, nothing else. Whatever sits below that
/// projection is trusted, since an attacker's SQL text never reaches below
/// the point where the aggregate view was built.
fn check_input_shape(mut node: &LogicalPlan) -> Result<(), String> {
    loop {
        match node {
            LogicalPlan::Filter(f) => {
                if has_subquery(&f.predicate) {
                    return Err("subqueries are only allowed below the aggregate".into());
                }
                node = &f.input;
            }
            LogicalPlan::SubqueryAlias(s) => node = &s.input,
            LogicalPlan::Projection(p) => {
                let user_key = p
                    .expr
                    .iter()
                    .any(|e| is_real_pseudonym(e, "user_key", "distinct_id"));
                let session_key = p
                    .expr
                    .iter()
                    .any(|e| is_real_pseudonym(e, "session_key", "session_id"));
                return if user_key && session_key {
                    Ok(())
                } else {
                    Err(
                        "aggregate directly over `events`: no sub-query is allowed under FROM"
                            .into(),
                    )
                };
            }
            _ => {
                return Err(
                    "aggregate directly over `events`: no sub-query is allowed under FROM".into(),
                );
            }
        }
    }
}

/// NSEC-01: `array_agg`, `string_agg`, `min`, `max`, `first_value` and
/// `last_value` on `user_key` or `session_key` would hand back the raw
/// pseudonym of people in groups that are too small to show, or let a
/// caller list pseudonyms to target one at a time in a later query. Only
/// `count(DISTINCT …)`, which is how the threshold itself counts people,
/// is allowed on these columns.
fn check_pseudonym_functions(exprs: &[Expr]) -> Result<(), String> {
    for e in exprs {
        let mut offender = None;
        e.apply(|x| {
            if let Expr::AggregateFunction(af) = x {
                let touches_pseudonym = af
                    .params
                    .args
                    .iter()
                    .any(|a| a.exists(|x| Ok(is_pseudonym_column(x))).unwrap_or(true));
                if touches_pseudonym
                    && !(af.func.name().eq_ignore_ascii_case("count") && af.params.distinct)
                {
                    offender = Some(af.func.name().to_owned());
                    return Ok(TreeNodeRecursion::Stop);
                }
            }
            Ok(TreeNodeRecursion::Continue)
        })
        .map_err(|e| e.to_string())?;
        if let Some(name) = offender {
            return Err(format!(
                "only count(DISTINCT …) is allowed on `user_key` or `session_key`, not {name}(…)"
            ));
        }
    }
    Ok(())
}

/// Check that `plan` only returns aggregates, then add the group-size
/// threshold. The error is a message for the caller.
pub(crate) fn guard(plan: LogicalPlan) -> Result<LogicalPlan, String> {
    let mut window = false;
    plan.apply_with_subqueries(|n| {
        if matches!(n, LogicalPlan::Window(_)) {
            window = true;
            return Ok(TreeNodeRecursion::Stop);
        }
        Ok(TreeNodeRecursion::Continue)
    })
    .map_err(|e| e.to_string())?;
    if window {
        return Err("window functions (OVER) are not allowed: aggregate with GROUP BY".into());
    }
    let mut node = &plan;
    loop {
        match node {
            LogicalPlan::Aggregate(a) => {
                check_input_shape(&a.input)?;
                check_pseudonym_functions(&a.aggr_expr)?;
                break;
            }
            LogicalPlan::Projection(_)
            | LogicalPlan::Filter(_)
            | LogicalPlan::Sort(_)
            | LogicalPlan::Limit(_)
            | LogicalPlan::SubqueryAlias(_) => {
                if node.expressions().iter().any(has_subquery) {
                    return Err("subqueries are only allowed below the aggregate".into());
                }
                let inputs = node.inputs();
                let [input] = inputs.as_slice() else {
                    return Err("aggregate the rows (GROUP BY, count, sum, avg)".into());
                };
                node = input;
            }
            _ => return Err("aggregate the rows (GROUP BY, count, sum, avg)".into()),
        }
    }
    plan.transform_down(|n| match n {
        LogicalPlan::Aggregate(a) => Ok(Transformed::new(
            threshold(a)?,
            true,
            TreeNodeRecursion::Stop,
        )),
        other => Ok(Transformed::no(other)),
    })
    .map(|t| t.data)
    .map_err(|e| {
        if e.to_string().contains("user_key") {
            "aggregate directly over `events` (its `user_key` must reach the GROUP BY)".into()
        } else {
            e.to_string()
        }
    })
}

/// The same aggregate, without the groups of fewer than [`MIN_USERS`].
fn threshold(a: Aggregate) -> datafusion::error::Result<LogicalPlan> {
    let users = normalize_col(count_distinct(col("user_key")), &a.input)?.alias(USERS_COL);
    let mut aggr = a.aggr_expr.clone();
    aggr.push(users);
    let wide = Aggregate::try_new(a.input.clone(), a.group_expr.clone(), aggr)?;
    let kept = Filter::try_new(
        col(USERS_COL).gt_eq(lit(MIN_USERS)),
        Arc::new(LogicalPlan::Aggregate(wide)),
    )?;
    let columns = a.schema.columns().into_iter().map(Expr::Column).collect();
    Ok(LogicalPlan::Projection(Projection::try_new(
        columns,
        Arc::new(LogicalPlan::Filter(kept)),
    )?))
}
