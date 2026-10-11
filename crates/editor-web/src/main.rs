use leptos::prelude::*;
use leptos::mount::mount_to_body;
use web_sys::HtmlTextAreaElement;
use js_sys::JsString;
use wasm_bindgen::JsCast;
use gloo_net::http::Request;
use gloo_timers::callback::Timeout;
use std::{cell::RefCell, rc::Rc};
use editor_core::{Chapter,SaveChapter,MAX_CHAPTER_CHARS,CHAPTER_LENGTH_ERROR};

// UTF-16 positions come from the browser. The full chapter stays in JS;
// only the selected substring is copied to Rust/WASM.
fn valid_selection_boundary(text: &JsString, index: u32) -> bool {
    if index == 0 || index == text.length() { return true; }
    // Reject positions inside a surrogate pair, even when set programmatically.
    text.slice(index - 1, index).is_valid_utf16()
        || text.slice(index, index + 1).is_valid_utf16()
}

fn selection_text(el: &HtmlTextAreaElement, start: u32, end: u32) -> Option<String> {
    let raw = js_sys::Reflect::get(el.as_ref(), &wasm_bindgen::JsValue::from_str("value")).ok()?;
    if !raw.is_string() { return None; }
    let value: JsString = raw.unchecked_into();
    if start > end || end > value.length()
        || !valid_selection_boundary(&value, start)
        || !valid_selection_boundary(&value, end) {
        return None;
    }
    let selected = value.slice(start, end);
    if !selected.is_valid_utf16() { return None; }
    Some(String::from(selected))
}

#[component]
fn App() -> impl IntoView {
    let area = NodeRef::<leptos::html::Textarea>::new();
    let (title,set_title)=signal("Новая глава".to_string());
    let (revision,set_revision)=signal(0u64);
    let (dirty,set_dirty)=signal(false);
    let (busy,set_busy)=signal(false);
    let (ready,set_ready)=signal(false);
    let (count_pending,set_count_pending)=signal(false);
    let (message,set_message)=signal("Загрузка...".to_string());
    let (preview,set_preview)=signal(String::new());
    let (mode,set_mode)=signal(0u8);
    let (text_len,set_text_len)=signal(0usize);
    let (edit_epoch,set_edit_epoch)=signal(0u64);
    // A pending timer is cancelled when another character arrives.
    let count_timer: Rc<RefCell<Option<Timeout>>> = Rc::new(RefCell::new(None));
    let textarea = move || area.get().map(|el| <_ as AsRef<HtmlTextAreaElement>>::as_ref(&el).clone());
    let recount: Rc<dyn Fn()> = Rc::new(move || {
        if !count_pending.get_untracked() { set_count_pending.set(true); }
        count_timer.borrow_mut().take();
        let timer = Timeout::new(300, move || {
            if let Some(el) = textarea() {
                set_text_len.set(el.value().chars().count());
            }
            set_count_pending.set(false);
        });
        count_timer.borrow_mut().replace(timer);
    });
    let recount_after_format = Rc::clone(&recount);
    let recount_after_input = Rc::clone(&recount);
    let render_preview = move || {
        if let Some(el)=textarea() {
            let text=el.value();
            let parser=pulldown_cmark::Parser::new_ext(&text, pulldown_cmark::Options::all());
            let mut html=String::new();
            pulldown_cmark::html::push_html(&mut html,parser);
            set_preview.set(ammonia::clean(&html));
        }
    };
    let edit: Rc<dyn Fn(&'static str, &'static str)> = Rc::new(move |prefix: &'static str, suffix: &'static str| {
        if !ready.get_untracked() { return; }
        if let Some(el)=textarea(){
            let start=el.selection_start().ok().flatten().unwrap_or(0);
            let end=el.selection_end().ok().flatten().unwrap_or(start);
            // UTF-16 slicing runs in JS; only the actual selection crosses into WASM.
            let Some(selected) = selection_text(&el, start, end) else { return };
            let replacement=format!("{prefix}{selected}{suffix}");
            // Use the native undo buffer when available. execCommand is deprecated,
            // so the browser regression test protects this compatibility path.
            let _=el.focus();
            let native_undo=web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.unchecked_into::<web_sys::HtmlDocument>().exec_command_with_show_ui_and_value("insertText",false,&replacement).ok())
                .unwrap_or(false);
            let inserted = if native_undo { true } else {
                el.set_range_text_with_start_and_end_and_mode(&replacement,start,end,"select").is_ok()
            };
            if !inserted { return; }
            set_dirty.set(true);
            set_edit_epoch.update(|n| *n += 1);
            recount_after_format();
            let _=el.focus();
        }
    });
    let edit_bold = Rc::clone(&edit);
    let edit_italic = Rc::clone(&edit);
    let edit_heading = Rc::clone(&edit);
    let edit_quote = Rc::clone(&edit);
    let edit_list = Rc::clone(&edit);
    let edit_link = Rc::clone(&edit);
    let edit_image = Rc::clone(&edit);
    let load=move || {
        let load_epoch=edit_epoch.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            match Request::get("/api/chapter").send().await {
                Ok(response) => match response.json::<Chapter>().await {
                    Ok(doc) => {
                        // Never replace a local edit with a late network response.
                        // The form remains read-only until the document is authoritative.
                        if edit_epoch.get_untracked()!=load_epoch {
                            set_message.set("Текст изменился во время загрузки. Мы не перезаписали его; скопируйте свои изменения и обновите страницу.".into());
                            return;
                        }
                        set_title.set(doc.title);
                        set_revision.set(doc.revision);
                        if let Some(el)=textarea(){el.set_value(&doc.markdown);}
                        set_text_len.set(doc.markdown.chars().count());
                        set_dirty.set(false);
                        set_ready.set(true);
                        set_message.set("Глава загружена".into());
                    }
                    Err(e) => set_message.set(format!("Ошибка чтения: {e}")),
                },
                Err(e) => set_message.set(format!("Ошибка подключения: {e}")),
            }
        });
    };
    Effect::new(move |_| {load();});
    let save=move |_| {
        if busy.get_untracked() || !ready.get_untracked(){return;}
        let Some(el)=textarea() else {return};
        let body=SaveChapter{title:title.get_untracked(),markdown:el.value(),expected_revision:revision.get_untracked()};
        let exact_char_count=body.markdown.chars().count();
        set_text_len.set(exact_char_count);
        if exact_char_count > MAX_CHAPTER_CHARS {
            set_message.set(CHAPTER_LENGTH_ERROR.to_string());
            return;
        }
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
        if !ready.get_untracked() { return; }
        if let Some(el)=textarea(){
            let pattern="Тест текста на русском языке. 世界 🌍\n";
            let pattern_chars=pattern.chars().count();
            let mut sample=String::with_capacity(len*3);
            for _ in 0..(len/pattern_chars) {sample.push_str(pattern);}
            sample.extend(pattern.chars().take(len%pattern_chars));
            let chars=sample.chars().count();
            el.set_value(&sample);set_text_len.set(chars);set_dirty.set(true);
            set_edit_epoch.update(|n| *n += 1);
            set_message.set(format!("Подготовлен образец: {len} символов. Сохранение — отдельно."));
        }
    };
    view! {
      <div class="shell">
       <header class="top"><h1>"BookaRust — редактор главы"</h1><span class="muted">{move || if dirty.get() {"● Не сохранено"} else {"✓ Без изменений"}}</span></header>
       <main class="form">
         <label>"Название главы"<input type="text" readonly=move || !ready.get() prop:value=move || title.get() on:input=move |ev| {set_title.set(event_target_value(&ev));set_dirty.set(true);set_edit_epoch.update(|n| *n += 1);}/></label>
         <div class="toolbar">
           <button type="button" disabled=move || !ready.get() on:click=move |_| edit_bold("**","**")>"Жирный"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_| edit_italic("*","*")>"Курсив"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_| edit_heading("## ","")>"Заголовок"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_| edit_quote("> ","")>"Цитата"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_| edit_list("- ","")>"Список"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_| edit_link("[","](https://example.com)")>"Ссылка"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_| edit_image("![","](https://example.com/image.png)")>"Изображение"</button>
         </div>
         <div class="row">
           <button type="button" disabled=move || !ready.get() on:click=move |_|set_mode.set(0)>"Текст"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_|{render_preview();set_mode.set(1)}>"Предпросмотр"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_|{render_preview();set_mode.set(2)}>"Два окна"</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_|make_sample(5_000)>"5 тыс."</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_|make_sample(10_000)>"10 тыс."</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_|make_sample(20_000)>"20 тыс."</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_|make_sample(50_000)>"50 тыс."</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_|make_sample(150_000)>"150 тыс."</button>
           <button type="button" disabled=move || !ready.get() on:click=move |_|make_sample(1_000_000)>"1 млн"</button>
           <span class=move || { if text_len.get() > MAX_CHAPTER_CHARS && !count_pending.get() { "error" } else { "muted" } }>{move || if count_pending.get() { "Подсчёт символов…".to_string() } else { format!("Символов: {} из {}",text_len.get(),MAX_CHAPTER_CHARS) }}</span>
         </div>
         <div class=move || if mode.get()==2 {"columns"} else {"columns single"}>
           <textarea node_ref=area readonly=move || !ready.get() style:display=move || if mode.get()==1 {"none"} else {"block"}
             aria-label="Текст главы"
             on:input=move |_| {
                // Hot path is O(1) in chapter length: the browser owns the text.
                set_dirty.set(true);
                set_edit_epoch.update(|n| *n += 1);
                recount_after_input();
             }
           ></textarea>
           <article class="preview" style:display=move ||if mode.get()==0 {"none"}else{"block"}
             inner_html=move ||preview.get()></article>
         </div>
         <div class="row"><button class="primary" type="button" disabled=move || { !ready.get() || busy.get() || (text_len.get() > MAX_CHAPTER_CHARS && !count_pending.get()) } on:click=save>"Сохранить"</button><span aria-live="polite">{move ||message.get()}</span></div>
         <p class="muted">"Обычная глава — 5–10 тыс. символов, максимум 50 тыс. Кнопки 150 тыс. и 1 млн — только для испытания нагрузки; такие тексты сохранить нельзя. Проверочный прототип для локального запуска."</p>
       </main>
      </div>
    }
}
fn main(){mount_to_body(App)}
