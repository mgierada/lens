//! Self-contained interactive graph page (cytoscape.js) for a report.

use anyhow::Result;

use crate::model::Report;

/// Page template shared with the nvim plugin, which fills it the same way.
pub const GRAPH_TEMPLATE: &str = include_str!("../assets/graph.html");
const REPORT_PLACEHOLDER: &str = "/*LENS_REPORT*/null";
const FOCUS_PLACEHOLDER: &str = "/*LENS_FOCUS*/null";

/// Render the graph page with the report embedded, optionally focused on one cluster.
pub fn render_graph(report: &Report, focus: Option<usize>) -> Result<String> {
    // `<` only occurs inside JSON strings; escaping it keeps `</script>` or `<!--` in code
    // snippets from ending the script element.
    let json = serde_json::to_string(report)?.replace('<', "\\u003c");
    let focus = focus.map_or_else(|| "null".to_string(), |f| f.to_string());
    Ok(GRAPH_TEMPLATE
        .replace(REPORT_PLACEHOLDER, &json)
        .replace(FOCUS_PLACEHOLDER, &focus))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_has_placeholders() {
        assert!(GRAPH_TEMPLATE.contains(REPORT_PLACEHOLDER));
        assert!(GRAPH_TEMPLATE.contains(FOCUS_PLACEHOLDER));
    }
}
