use front_rust_ui_compare::{run, Scene};
use leptos::prelude::*;
fn render(scene: &Scene) -> String {
    let items = scene.rects.clone();
    view! {
        <main data-benchmark="ui-compare">
            <h1>"Rectangles"</h1>
            <section role="group" aria-label="Rectangles">
                {items.into_iter().map(|r| {
                    let style=format!("position:absolute;left:{}px;top:{}px",r.x,r.y);
                    view! { <div class="rect" data-id=r.id data-x=r.x data-y=r.y
                        role="img" aria-label=format!("Rectangle {}",r.id)
                        style=style></div> }
                }).collect_view()}
            </section>
        </main>
    }.to_html()
}
fn main(){ run("leptos-ssr",render); }
