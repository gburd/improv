//! R subprocess runner.
//!
//! Invokes `Rscript -` (program on stdin). The driver embeds the JSON args
//! payload as an R string literal, decodes it with `jsonlite::fromJSON` when the
//! package is installed, else a tiny hand-rolled decoder (numbers/strings/
//! bools/lists) so **no CRAN dependency is required**. The user `body` runs with
//! `args` bound (a list) and must assign `result`; the driver encodes `result`
//! back to JSON and prints one `{"ok":..}` / `{"error":..}` envelope on stdout.
//! [`crate::runner`] enforces the wall-clock timeout with a hard kill.

use crate::marshal::value_to_json;
use crate::runner;
use crate::{ExtFnError, ExternalFn};
use improv_core_model::Value;
use serde_json::json;
use std::time::Duration;

/// Minimal R JSON codec + driver. `fromJSON`/`toJSON` from `jsonlite` are used
/// when present; otherwise the hand-rolled `.improv_from_json` / `.improv_to_json`
/// cover the scalar/list shapes this runtime marshals. `args` is bound to the
/// decoded arg list before the user body runs.
fn program(body: &str, payload_json: &str) -> String {
    let payload_lit = r_str_literal(payload_json);
    // NOTE: doubled braces are not needed here (no format! interpolation of `{`
    // inside the R source except via {payload_lit}/{body}); we build with
    // concat to keep the R verbatim and readable.
    let mut p = String::new();
    p.push_str(R_PRELUDE);
    p.push_str("._payload_str <- ");
    p.push_str(&payload_lit);
    p.push('\n');
    p.push_str("._payload <- .improv_from_json(._payload_str)\n");
    p.push_str("args <- ._payload$args\n");
    p.push_str("result <- NULL\n");
    p.push_str("._err <- tryCatch({\n");
    p.push_str(body);
    p.push_str("\n  NULL\n}, error = function(e) conditionMessage(e))\n");
    p.push_str(R_EPILOGUE);
    p
}

/// Run the function. Assumes arity/type checks already passed.
pub fn eval(f: &ExternalFn, args: &[Value], timeout: Duration) -> Result<Value, ExtFnError> {
    let payload = json!({
        "args": args.iter().map(value_to_json).collect::<Vec<_>>(),
    })
    .to_string();
    let prog = program(&f.body, &payload);
    // `--vanilla` = no site/user profiles, no saved workspace, no history.
    let out = runner::run_interpreter("Rscript", &["--vanilla", "-"], &prog, timeout, "R")?;
    runner::parse_envelope(&out.stdout, &out.stderr, f.return_type, "R")
}

/// Encode a Rust string as an R string literal (R accepts JSON's double-quoted
/// escapes for our characters).
fn r_str_literal(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

/// JSON decode/encode helpers in pure base R (fallback when `jsonlite` absent).
/// Uses R's own parser (`parse`/`eval` on a JSON-ish literal is unsafe, so we
/// hand-tokenize). Objects become named lists, arrays become unnamed lists.
const R_PRELUDE: &str = r#"
.improv_from_json <- function(s) {
  if (requireNamespace("jsonlite", quietly = TRUE)) {
    return(jsonlite::fromJSON(s, simplifyVector = FALSE))
  }
  chars <- strsplit(s, "", fixed = TRUE)[[1]]
  i <- 1L
  n <- length(chars)
  skip_ws <- function() { while (i <= n && chars[i] %in% c(" ", "\t", "\n", "\r")) i <<- i + 1L }
  parse_val <- function() {
    skip_ws()
    c <- chars[i]
    if (c == "{") return(parse_obj())
    if (c == "[") return(parse_arr())
    if (c == "\"") return(parse_str())
    if (c == "t") { i <<- i + 4L; return(TRUE) }
    if (c == "f") { i <<- i + 5L; return(FALSE) }
    if (c == "n") { i <<- i + 4L; return(NULL) }
    parse_num()
  }
  parse_str <- function() {
    i <<- i + 1L
    out <- ""
    while (i <= n && chars[i] != "\"") {
      if (chars[i] == "\\") {
        i <<- i + 1L
        e <- chars[i]
        out <- paste0(out, switch(e, n = "\n", t = "\t", r = "\r", e))
      } else out <- paste0(out, chars[i])
      i <<- i + 1L
    }
    i <<- i + 1L
    out
  }
  parse_num <- function() {
    start <- i
    while (i <= n && (chars[i] %in% c("-", "+", ".", "e", "E") || grepl("[0-9]", chars[i]))) i <<- i + 1L
    as.numeric(paste(chars[start:(i - 1L)], collapse = ""))
  }
  parse_arr <- function() {
    i <<- i + 1L
    out <- list()
    skip_ws()
    if (chars[i] == "]") { i <<- i + 1L; return(out) }
    repeat {
      out[[length(out) + 1L]] <- parse_val()
      skip_ws()
      if (chars[i] == ",") { i <<- i + 1L; next }
      if (chars[i] == "]") { i <<- i + 1L; break }
    }
    out
  }
  parse_obj <- function() {
    i <<- i + 1L
    out <- list()
    skip_ws()
    if (chars[i] == "}") { i <<- i + 1L; return(out) }
    repeat {
      skip_ws()
      k <- parse_str()
      skip_ws(); i <<- i + 1L  # skip ':'
      out[[k]] <- parse_val()
      skip_ws()
      if (chars[i] == ",") { i <<- i + 1L; next }
      if (chars[i] == "}") { i <<- i + 1L; break }
    }
    out
  }
  parse_val()
}

.improv_to_json <- function(x) {
  if (requireNamespace("jsonlite", quietly = TRUE)) {
    return(jsonlite::toJSON(x, auto_unbox = TRUE, digits = NA))
  }
  if (is.null(x)) return("null")
  if (is.logical(x) && length(x) == 1L) return(if (x) "true" else "false")
  if (is.numeric(x) && length(x) == 1L) return(format(x, scientific = FALSE, trim = TRUE))
  if (is.character(x) && length(x) == 1L) {
    esc <- gsub("\\\\", "\\\\\\\\", x)
    esc <- gsub("\"", "\\\\\"", esc)
    esc <- gsub("\n", "\\\\n", esc)
    return(paste0("\"", esc, "\""))
  }
  if (is.list(x) && !is.null(names(x))) {
    parts <- vapply(seq_along(x), function(j) paste0("\"", names(x)[j], "\":", .improv_to_json(x[[j]])), character(1))
    return(paste0("{", paste(parts, collapse = ","), "}"))
  }
  parts <- vapply(x, .improv_to_json, character(1))
  paste0("[", paste(parts, collapse = ","), "]")
}
"#;

/// After the body runs, emit the envelope. `._err` is non-NULL on a caught
/// error; a NULL `result` becomes a "did not set result" error.
const R_EPILOGUE: &str = r#"
if (!is.null(._err)) {
  cat(paste0("{\"error\":", .improv_to_json(._err), "}\n"))
} else if (is.null(result)) {
  cat("{\"error\":\"function body did not set `result`\"}\n")
} else {
  cat(paste0("{\"ok\":", .improv_to_json(result), "}\n"))
}
"#;
