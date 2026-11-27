//! The motion rules of `app.css` (#552): everything that moves sits inside
//! `prefers-reduced-motion: no-preference`, nothing animates a layout
//! property, and the layers stay flat and closed. A small brace walker
//! reads the stylesheet; it knows comments, strings and parentheses, which
//! is all `app.css` uses.

/// One declaration and the preludes of the blocks around it, outermost
/// first.
struct Decl {
    prop: String,
    value: String,
    ctx: Vec<String>,
}

/// The stylesheet read as declarations and blocks (each block's prelude
/// with the preludes around it). An error when the braces do not balance.
/// A block: its prelude and the preludes around it.
type Block = (String, Vec<String>);

fn walk(css: &str) -> Result<(Vec<Decl>, Vec<Block>), String> {
    let mut decls = Vec::new();
    let mut blocks = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut quote: Option<char> = None;
    let mut parens = 0usize;
    let mut chars = css.chars().peekable();
    let flush = |buf: &mut String, stack: &[String], decls: &mut Vec<Decl>| {
        let text = buf.trim();
        if !stack.is_empty()
            && let Some((p, v)) = text.split_once(':')
        {
            decls.push(Decl {
                prop: p.trim().to_ascii_lowercase(),
                value: v.trim().to_string(),
                ctx: stack.to_vec(),
            });
        }
        buf.clear();
    };
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            buf.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            '"' | '\'' => {
                quote = Some(c);
                buf.push(c);
            }
            '(' => {
                parens += 1;
                buf.push(c);
            }
            ')' => {
                parens = parens.saturating_sub(1);
                buf.push(c);
            }
            ';' if parens == 0 => flush(&mut buf, &stack, &mut decls),
            '{' => {
                let prelude = buf.split_whitespace().collect::<Vec<_>>().join(" ");
                blocks.push((prelude.clone(), stack.clone()));
                stack.push(prelude);
                buf.clear();
            }
            '}' => {
                flush(&mut buf, &stack, &mut decls);
                if stack.pop().is_none() {
                    return Err("a `}` closes nothing".into());
                }
            }
            _ => buf.push(c),
        }
    }
    if !stack.is_empty() {
        return Err(format!("{} block(s) left open: {stack:?}", stack.len()));
    }
    Ok((decls, blocks))
}

const NO_PREFERENCE: &str = "prefers-reduced-motion: no-preference";

/// The animations and transitions that move something outside a
/// `no-preference` block (`none` switches motion off, so it may sit
/// anywhere).
fn unguarded(decls: &[Decl]) -> Vec<String> {
    decls
        .iter()
        .filter(|d| d.prop.starts_with("animation") || d.prop.starts_with("transition"))
        .filter(|d| d.value.trim_end_matches("!important").trim() != "none")
        .filter(|d| !d.ctx.iter().any(|p| p.contains(NO_PREFERENCE)))
        .map(|d| {
            format!(
                "{} {{ {} }}",
                d.ctx.last().map_or("", String::as_str),
                d.prop
            )
        })
        .collect()
}

fn is_layout(prop: &str) -> bool {
    let p = prop.trim_start_matches("min-").trim_start_matches("max-");
    matches!(
        p,
        "all"
            | "width"
            | "height"
            | "top"
            | "left"
            | "right"
            | "bottom"
            | "inline-size"
            | "block-size"
    ) || p.starts_with("inset")
        || p.starts_with("margin")
        || p.starts_with("padding")
}

/// The properties a `transition` (or `transition-property`) value names.
fn transitioned(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut seg = String::new();
    let mut segments = Vec::new();
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                segments.push(std::mem::take(&mut seg));
                continue;
            }
            _ => {}
        }
        seg.push(c);
    }
    segments.push(seg);
    for s in segments {
        let prop = s.split_whitespace().find(|t| {
            let t = t.to_ascii_lowercase();
            let time = t.starts_with(|c: char| c.is_ascii_digit() || c == '.') && t.ends_with('s');
            let easing = t.contains('(')
                || matches!(
                    t.as_str(),
                    "ease"
                        | "ease-in"
                        | "ease-out"
                        | "ease-in-out"
                        | "linear"
                        | "step-start"
                        | "step-end"
                );
            !time && !easing && !matches!(t.as_str(), "allow-discrete" | "normal" | "!important")
        });
        if let Some(p) = prop {
            out.push(p.to_ascii_lowercase());
        }
    }
    out
}

/// The layout properties something animates: a transition naming one, or
/// a keyframe setting one. The height of an opening disclosure
/// (`::details-content`, §3.4) is the one allowed.
fn layout_animated(decls: &[Decl]) -> Vec<String> {
    let mut bad = Vec::new();
    for d in decls {
        let sel = d.ctx.last().map_or("", String::as_str);
        let props = if d.prop == "transition" || d.prop == "transition-property" {
            transitioned(&d.value)
        } else if d.ctx.iter().any(|p| p.starts_with("@keyframes")) {
            vec![d.prop.clone()]
        } else {
            continue;
        };
        for p in props {
            let disclosure = p == "block-size" && sel.contains("::details-content");
            if is_layout(&p) && !disclosure {
                bad.push(format!("{sel} {{ {}: {} }}", d.prop, d.value));
            }
        }
    }
    bad
}

#[test]
fn everything_that_moves_waits_for_no_preference() {
    let (decls, blocks) = walk(crate::CSS).unwrap_or_else(|e| panic!("{e}"));
    // Everything that moves, the logo included (#591), is in the motion
    // layer: no exception list.
    let new = unguarded(&decls);
    assert!(new.is_empty(), "motion outside no-preference: {new:?}");
    let vt: Vec<&Vec<String>> = blocks
        .iter()
        .filter(|(p, _)| p.starts_with("@view-transition"))
        .map(|(_, ctx)| ctx)
        .collect();
    assert!(!vt.is_empty(), "no cross-document view transition");
    assert!(
        vt.iter()
            .all(|ctx| ctx.iter().any(|p| p.contains(NO_PREFERENCE)))
    );
    // The reduce rule still switches everything off, last layer first.
    assert!(decls.iter().any(|d| {
        d.prop == "animation"
            && d.value.contains("none !important")
            && d.ctx.iter().any(|p| p == "@layer utilities")
            && d.ctx
                .iter()
                .any(|p| p.contains("prefers-reduced-motion: reduce"))
    }));
    // The walker does catch one added outside.
    let (outside, _) = walk("@layer motion { .x { transition: opacity 1s; } }").unwrap_or_default();
    let (inside, _) = walk(
        "@layer motion { @media (prefers-reduced-motion: no-preference) { .x { transition: opacity 1s; } } }",
    )
    .unwrap_or_default();
    assert!(unguarded(&outside).len() > unguarded(&inside).len());
}

#[test]
fn nothing_animates_a_layout_property() {
    let (decls, _) = walk(crate::CSS).unwrap_or_else(|e| panic!("{e}"));
    let bad = layout_animated(&decls);
    assert!(bad.is_empty(), "layout animated: {bad:?}");
    let (probe, _) =
        walk("@keyframes k { to { margin-top: 4px; } } .x { transition: opacity .2s, width .2s; }")
            .unwrap_or_default();
    assert!(layout_animated(&probe).len() > 1);
}

#[test]
fn the_stylesheet_closes_and_its_layers_sit_at_the_top() {
    let (_, blocks) = walk(crate::CSS).unwrap_or_else(|e| panic!("{e}"));
    let nested: Vec<&String> = blocks
        .iter()
        .filter(|(p, ctx)| p.starts_with("@layer") && !ctx.is_empty())
        .map(|(p, _)| p)
        .collect();
    assert!(nested.is_empty(), "nested @layer: {nested:?}");
    assert!(blocks.iter().any(|(p, _)| p == "@layer motion"));
    assert!(walk(".a { color: red; ").is_err() && walk(".a { } }").is_err());
}
