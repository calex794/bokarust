use front_rust_ui_compare::{run, Scene};
fn render(scene: &Scene) -> String {
    let mut html = String::from(
        r#"<main data-benchmark="ui-compare"><h1>Rectangles</h1><section role="group" aria-label="Rectangles">"#
    );
    for r in &scene.rects {
        html.push_str(&format!(
            r#"<div class="rect" data-id="{}" data-x="{}" data-y="{}" role="img" aria-label="Rectangle {}" style="position:absolute;left:{}px;top:{}px"></div>"#,
            r.id,r.x,r.y,r.id,r.x,r.y
        ));
    }
    html.push_str("</section></main>");
    html
}
fn main(){ run("rust-baseline", render); }
