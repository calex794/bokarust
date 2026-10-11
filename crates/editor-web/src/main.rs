use leptos::prelude::*;
use leptos::mount::mount_to_body;
use web_sys::HtmlTextAreaElement;
use gloo_net::http::Request;
use editor_core::{Chapter,SaveChapter};

#[component]
fn App() -> impl IntoView {
    let area = NodeRef::<leptos::html::Textarea>::new();
    let (title,set_title)=signal("Новая глава".to_string());
    let (revision,set_revision)=signal(0u64);
    let (dirty,set_dirty)=signal(false);
    let (busy,set_busy)=signal(false);
    let (message,set_message)=signal("Загрузка...".to_string());
    let (preview,set_preview)=signal(String::new());
    let (mode,set_mode)=signal(0u8);
    let (text_len,set_text_len)=signal(0usize);
    let (edit_epoch,set_edit_epoch)=signal(0u64);

    let textarea = move || area.get().map(|el| <_ as AsRef<HtmlTextAreaElement>>::as_ref(&el).clone());
    let render_preview = move || {
        if let Some(el)=textarea() {
            let text=el.value();
            let parser=pulldown_cmark::Parser::new_ext(&text, pulldown_cmark::Options::all());
            let mut html=String::new();
            pulldown_cmark::html::push_html(&mut html,parser);
            set_preview.set(ammonia::clean(&html));
        }
    };
    let edit = move |prefix: &'static str, suffix: &'static str| {
        if let Some(el)=textarea(){
            let start=el.selection_start().ok().flatten().unwrap_or(0);
            let end=el.selection_end().ok().flatten().unwrap_or(start);
            let value=el.value();
            // Textarea selection indices are UTF-16. Browser selection is not byte indexing.
            let chars:Vec<u16>=value.encode_utf16().collect();
            let a=(start as usize).min(chars.len());let b=(end as usize).min(chars.len());
            let selected=String::from_utf16_lossy(&chars[a..b]);
            let replacement=format!("{prefix}{selected}{suffix}");
            let _=el.set_range_text_with_start_and_end_and_mode(&replacement,start,end,"select");
            set_dirty.set(true);set_text_len.set(el.text_length() as usize);set_edit_epoch.update(|n| *n += 1);
            let _=el.focus();
        }
    };
    let load=move || wasm_bindgen_futures::spawn_local(async move {
        match Request::get("/api/chapter").send().await {
            Ok(response)=>match response.json::<Chapter>().await {
                Ok(doc)=>{set_title.set(doc.title);set_revision.set(doc.revision);if let Some(el)=textarea(){el.set_value(&doc.markdown);}set_text_len.set(doc.markdown.encode_utf16().count());set_dirty.set(false);set_message.set("Глава загружена".into());},
                Err(e)=>set_message.set(format!("Ошибка чтения: {e}"))
            },
            Err(e)=>set_message.set(format!("Ошибка подключения: {e}"))
        };
    });
    Effect::new(move |_| {load();});
    let save=move |_| {
        if busy.get_untracked(){return;}
        let Some(el)=textarea() else {return};
        let body=SaveChapter{title:title.get_untracked(),markdown:el.value(),expected_revision:revision.get_untracked()};
        let saving_epoch = edit_epoch.get_untracked();
        set_busy.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let result=async {
                let response=Request::put("/api/chapter").json(&body).map_err(|e|e.to_string())?.send().await.map_err(|e|e.to_string())?;
                if !response.ok(){return Err(format!("Ошибка {}: {}",response.status(),response.text().await.unwrap_or_default()));}
                let doc=response.json::<Chapter>().await.map_err(|e|e.to_string())?;
                Ok(doc)
            }.await;
            match result {
                Ok(doc)=>{set_revision.set(doc.revision);if edit_epoch.get_untracked()==saving_epoch {set_dirty.set(false);set_message.set(format!("Сохранено: версия {}",doc.revision));} else {set_dirty.set(true);set_message.set(format!("Версия {} сохранена, но есть более новые несохранённые изменения",doc.revision));}},
                Err(e)=>set_message.set(e)
            }
            set_busy.set(false);
        });
    };
    let make_sample=move |len:usize| {
        if let Some(el)=textarea(){
            let pattern="Тест текста на русском языке. 世界 🌍\n";
            let pattern_chars=pattern.chars().count();
            let mut sample=String::with_capacity(len*3);
            for _ in 0..(len/pattern_chars) {sample.push_str(pattern);}
            sample.extend(pattern.chars().take(len%pattern_chars));
            let length_utf16=sample.encode_utf16().count();
            el.set_value(&sample);set_text_len.set(length_utf16);set_dirty.set(true);
            set_edit_epoch.update(|n| *n += 1);
            set_message.set(format!("Подготовлен образец: {len} символов. Сохранение — отдельно."));
        }
    };
    view! {
      <div class="shell">
       <header class="top"><h1>"BookaRust — редактор главы"</h1><span class="muted">{move || if dirty.get() {"● Не сохранено"} else {"✓ Без изменений"}}</span></header>
       <main class="form">
         <label>"Название главы"<input type="text" prop:value=move || title.get() on:input=move |ev| {set_title.set(event_target_value(&ev));set_dirty.set(true);set_edit_epoch.update(|n| *n += 1);}/></label>
         <div class="toolbar">
           <button type="button" on:click=move |_| edit("**","**")>"Жирный"</button>
           <button type="button" on:click=move |_| edit("*","*")>"Курсив"</button>
           <button type="button" on:click=move |_| edit("## ","")>"Заголовок"</button>
           <button type="button" on:click=move |_| edit("> ","")>"Цитата"</button>
           <button type="button" on:click=move |_| edit("- ","")>"Список"</button>
           <button type="button" on:click=move |_| edit("[","](https://example.com)")>"Ссылка"</button>
           <button type="button" on:click=move |_| edit("![","](https://example.com/image.png)")>"Изображение"</button>
         </div>
         <div class="row">
           <button type="button" on:click=move |_|set_mode.set(0)>"Текст"</button>
           <button type="button" on:click=move |_|{render_preview();set_mode.set(1)}>"Предпросмотр"</button>
           <button type="button" on:click=move |_|{render_preview();set_mode.set(2)}>"Два окна"</button>
           <button type="button" on:click=move |_|make_sample(20_000)>"20 тыс."</button>
           <button type="button" on:click=move |_|make_sample(150_000)>"150 тыс."</button>
           <button type="button" on:click=move |_|make_sample(1_000_000)>"1 млн"</button>
           <span class="muted">{move ||format!("Длина UTF-16: {}",text_len.get())}</span>
         </div>
         <div class=move || if mode.get()==2 {"columns"} else {"columns single"}>
           <textarea node_ref=area style:display=move || if mode.get()==1 {"none"} else {"block"}
             aria-label="Текст главы"
             on:input=move |_| {set_dirty.set(true);set_edit_epoch.update(|n| *n += 1);if let Some(el)=textarea(){set_text_len.set(el.text_length() as usize)}}
           ></textarea>
           <article class="preview" style:display=move ||if mode.get()==0 {"none"}else{"block"}
             inner_html=move ||preview.get()></article>
         </div>
         <div class="row"><button class="primary" type="button" disabled=move ||busy.get() on:click=save>"Сохранить"</button><span aria-live="polite">{move ||message.get()}</span></div>
         <p class="muted">"Проверочный прототип. Только локальный запуск, без входа пользователей и защиты данных."</p>
       </main>
      </div>
    }
}
fn main(){mount_to_body(App)}
