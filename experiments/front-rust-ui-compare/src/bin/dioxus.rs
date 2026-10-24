use front_rust_ui_compare::{run, Scene};
use dioxus::prelude::*;
fn render(scene: &Scene) -> String {
    let items=scene.rects.clone();
    dioxus_ssr::render_element(rsx! {
        main { "data-benchmark": "ui-compare",
            h1 { "Rectangles" }
            section { role: "group", "aria-label": "Rectangles",
                for r in items {
                    div {
                        key: "{r.id}",
                        class: "rect",
                        "data-id": "{r.id}",
                        "data-x": "{r.x}",
                        "data-y": "{r.y}",
                        role: "img",
                        "aria-label": "Rectangle {r.id}",
                        style: "position:absolute;left:{r.x}px;top:{r.y}px"
                    }
                }
            }
        }
    })
}
fn main(){ run("dioxus-ssr",render); }
