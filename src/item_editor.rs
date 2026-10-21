use crate::gui::{App, Message};
use fast_pack::compat::custom_items::{self, Item, Outline, Rule};
use iced::widget::{button, checkbox, column, container, row, scrollable, space, text, text_input};
use iced::{Alignment, Border, Element, Length, Task};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
#[path = "item_editor_tests.rs"]
mod tests;

#[derive(Clone, Debug)]
pub(crate) enum Event {
    Open,
    Enabled(bool),
    Search(String),
    Selected(String),
    Page(bool),
    Name(String),
    Inventory(String),
    Held(String),
    Dropped(String),
    Border(bool),
    Color(String),
    Width(u8),
    Save,
    Reset,
    Clear,
    Catalog(String, Arc<Vec<Item>>),
    Preview(u64, Result<image::RgbaImage, String>),
}

pub(crate) struct Editor {
    pub rules: Vec<Rule>,
    pub enabled: bool,
    open: bool,
    version: String,
    source: Option<PathBuf>,
    entries: Arc<Vec<Item>>,
    results: Vec<usize>,
    query: String,
    selected: Option<String>,
    page: usize,
    name: String,
    inventory: String,
    held: String,
    dropped: String,
    border: bool,
    rainbow: bool,
    color: String,
    width: u8,
    preview: Option<iced::widget::image::Handle>,
    preview_source: Option<image::RgbaImage>,
    revision: u64,
    note: String,
}

impl Editor {
    pub fn new(rules: Vec<Rule>) -> Self {
        Self {
            rules,
            enabled: false,
            open: false,
            version: String::new(),
            source: None,
            entries: Arc::new(Vec::new()),
            results: Vec::new(),
            query: String::new(),
            selected: None,
            page: 0,
            name: String::new(),
            inventory: "100".into(),
            held: "100".into(),
            dropped: "100".into(),
            border: false,
            rainbow: false,
            color: "#000000".into(),
            width: 1,
            preview: None,
            preview_source: None,
            revision: 0,
            note: String::new(),
        }
    }
    fn search(&mut self) {
        self.results = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, item)| item.matches(&self.query))
            .map(|(index, _)| index)
            .collect();
        self.page = 0;
    }
    pub fn sync(&mut self, version: &str, source: Option<PathBuf>) -> Task<Message> {
        if !self.open {
            return Task::none();
        }
        if self.version == version {
            if self.source != source {
                self.source = source.clone();
                if let Some(id) = self.selected.clone() {
                    return self.select(id, source);
                }
            }
            return Task::none();
        }
        self.version = version.into();
        self.source = source;
        self.entries = Arc::new(Vec::new());
        self.results.clear();
        self.selected = None;
        self.preview = None;
        self.preview_source = None;
        self.revision += 1;
        self.note = "正在加载该版本的物品…".into();
        let version = version.to_string();
        Task::perform(
            async move {
                let entries = custom_items::items(&version);
                Event::Catalog(version, entries)
            },
            |event| Message::Items(event),
        )
    }
    fn select(&mut self, id: String, source: Option<PathBuf>) -> Task<Message> {
        let Some(item) = self.entries.iter().find(|item| item.id == id).cloned() else {
            return Task::none();
        };
        self.selected = Some(id.clone());
        self.preview = None;
        self.preview_source = None;
        self.note.clear();
        self.revision += 1;
        let rule = self
            .rules
            .iter()
            .find(|rule| rule.version == self.version && rule.item == id)
            .cloned()
            .unwrap_or_default();
        self.name = rule.name.unwrap_or_default();
        self.inventory = rule.inventory_scale.to_string();
        self.held = rule.held_scale.to_string();
        self.dropped = rule.dropped_scale.to_string();
        self.border = rule.outline.is_some();
        self.rainbow = rule.outline.as_ref().is_some_and(|outline| outline.rainbow);
        self.color = fast_pack::compat::glint::color_hex(
            rule.outline
                .as_ref()
                .map_or([0; 3], |outline| outline.color),
        );
        self.width = rule.outline.map_or(1, |outline| outline.width);
        let revision = self.revision;
        Task::perform(
            async move {
                let result = fast_pack::tasks::run("生成物品预览", || {
                    custom_items::preview(
                        source.as_deref().unwrap_or(std::path::Path::new("")),
                        &item,
                    )
                })
                .map(|image| {
                    let side = image.width().min(image.height());
                    let image =
                        image::imageops::crop_imm(&image, 0, 0, image.width(), side).to_image();
                    if image.width() > 128 || image.height() > 128 {
                        image::imageops::thumbnail(&image, 128, 128)
                    } else {
                        image
                    }
                })
                .map_err(|error| format!("{error:#}"));
                Event::Preview(revision, result)
            },
            |event| Message::Items(event),
        )
    }
    pub fn update(
        &mut self,
        event: Event,
        version: &str,
        source: Option<PathBuf>,
    ) -> Task<Message> {
        match event {
            Event::Open => {
                self.open = !self.open;
                return self.sync(version, source);
            }
            Event::Enabled(value) => self.enabled = value,
            Event::Catalog(version, entries) if self.version == version => {
                self.entries = entries;
                self.search();
                self.note.clear();
                if let Some(item) = self
                    .entries
                    .iter()
                    .find(|item| item.id == "minecraft:diamond_sword")
                    .or_else(|| self.entries.first())
                {
                    return self.select(item.id.clone(), source);
                }
            }
            Event::Search(query) => {
                self.query = query;
                self.search()
            }
            Event::Selected(id) => return self.select(id, source),
            Event::Page(next) => {
                let last = self.results.len().saturating_sub(1) / 40;
                self.page = if next {
                    (self.page + 1).min(last)
                } else {
                    self.page.saturating_sub(1)
                }
            }
            Event::Name(value) => self.name = value,
            Event::Inventory(value) => self.inventory = value,
            Event::Held(value) => self.held = value,
            Event::Dropped(value) => self.dropped = value,
            Event::Border(value) => {
                self.border = value;
                self.refresh_preview();
            }
            Event::Color(value) => {
                self.color = value;
                self.refresh_preview();
            }
            Event::Width(value) => {
                self.width = value;
                self.refresh_preview();
            }
            Event::Preview(revision, result) if revision == self.revision => match result {
                Ok(image) => {
                    self.preview_source = Some(image);
                    self.refresh_preview();
                }
                Err(_) => self.note = "该物品使用特殊模型，暂无平面预览；设置仍可保存。".into(),
            },
            Event::Save => {
                let result = (|| -> anyhow::Result<Rule> {
                    let rule = Rule {
                        version: self.version.clone(),
                        item: self
                            .selected
                            .clone()
                            .ok_or_else(|| anyhow::anyhow!("请先选择物品"))?,
                        name: (!self.name.trim().is_empty()).then(|| self.name.trim().into()),
                        inventory_scale: self.inventory.parse()?,
                        held_scale: self.held.parse()?,
                        dropped_scale: self.dropped.parse()?,
                        outline: if self.border {
                            Some(Outline {
                                color: fast_pack::compat::glint::parse_color(&self.color)?,
                                width: self.width,
                                rainbow: self.rainbow,
                            })
                        } else {
                            None
                        },
                    };
                    custom_items::validate(&rule)?;
                    Ok(rule)
                })();
                match result {
                    Ok(rule) => {
                        self.rules.retain(|value| {
                            value.version != rule.version || value.item != rule.item
                        });
                        self.rules.push(rule);
                        self.enabled = true;
                        self.note = "已保存，转换时应用到这个目标版本。".into()
                    }
                    Err(error) => self.note = format!("设置未保存：{error}"),
                }
            }
            Event::Reset => {
                if let Some(id) = self.selected.clone() {
                    self.rules
                        .retain(|rule| rule.version != self.version || rule.item != id);
                    return self.select(id, source);
                }
            }
            Event::Clear => {
                self.rules.retain(|rule| rule.version != self.version);
                if let Some(id) = self.selected.clone() {
                    return self.select(id, source);
                }
            }
            _ => {}
        }
        Task::none()
    }
    pub fn applied(&self, version: &str) -> Vec<Rule> {
        if self.enabled {
            self.rules
                .iter()
                .filter(|rule| rule.version == version)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }
    pub fn applied_count(&self, version: &str) -> usize {
        if self.enabled {
            self.rules
                .iter()
                .filter(|rule| rule.version == version)
                .count()
        } else {
            0
        }
    }
    pub fn ready(&self, version: &str) -> bool {
        self.version == version && !self.entries.is_empty()
    }
    pub fn outline_color(&self) -> [u8; 3] {
        fast_pack::compat::glint::parse_color(&self.color).unwrap_or([0; 3])
    }
    pub fn outline_rainbow(&self) -> bool {
        self.rainbow
    }
    pub fn outline_palette_visible(&self) -> bool {
        self.open && self.selected.is_some() && self.border
    }
    pub fn set_outline_rainbow(&mut self, enabled: bool) {
        self.rainbow = enabled;
        self.border = true;
        self.refresh_preview();
    }
    pub fn set_outline_color(&mut self, color: [u8; 3]) {
        self.color = fast_pack::compat::glint::color_hex(color);
        self.border = true;
        self.refresh_preview();
    }
    fn refresh_preview(&mut self) {
        let Some(original) = &self.preview_source else {
            return;
        };
        let image = if self.border {
            fast_pack::compat::glint::parse_color(&self.color)
                .ok()
                .map(|color| {
                    custom_items::outlined(
                        original,
                        &Outline {
                            color,
                            width: self.width,
                            rainbow: self.rainbow,
                        },
                    )
                })
                .unwrap_or_else(|| original.clone())
        } else {
            original.clone()
        };
        self.preview = Some(iced::widget::image::Handle::from_rgba(
            image.width(),
            image.height(),
            image.into_raw(),
        ));
    }
}

impl App {
    pub(crate) fn items_view(&self) -> Element<'_, Message> {
        let editor = &self.item_editor;
        let c = self.colors();
        let count = editor
            .rules
            .iter()
            .filter(|rule| rule.version == self.target)
            .count();
        let mut content = column![
            row![
                self.title("物品自定义", 14),
                space().width(Length::Fill),
                checkbox(editor.enabled)
                    .label(if self.overlay_mode {
                        "制作时应用"
                    } else {
                        "转换时应用"
                    })
                    .text_size(13)
                    .size(17)
                    .on_toggle_maybe(
                        (!self.busy()).then_some(|value| Message::Items(Event::Enabled(value)))
                    ),
                self.action(
                    48,
                    if editor.open {
                        "收起"
                    } else {
                        "搜索与设置"
                    },
                    (!self.busy()).then_some(Message::Items(Event::Open)),
                    false
                ),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            self.label(format!(
                "目标 {} · {count} 项设置 · 配置自动保存在 C 盘",
                self.target
            ))
        ]
        .spacing(12);
        if editor.open {
            let mut search = text_input("搜索中文名、英文名或物品 ID", &editor.query)
                .size(14)
                .padding(10)
                .style(|_, status| self.input_style(status));
            if !self.busy() {
                search = search.on_input(|value| Message::Items(Event::Search(value)));
            }
            content = content.push(search);
            let mut list = column![].spacing(5);
            for &index in editor.results.iter().skip(editor.page * 40).take(40) {
                let item = &editor.entries[index];
                let selected = editor.selected.as_deref() == Some(&item.id);
                list = list.push(
                    button(
                        column![
                            text(&item.chinese).size(13),
                            text(&item.id).size(11).color(c.muted)
                        ]
                        .spacing(3),
                    )
                    .padding([8, 10])
                    .width(Length::Fill)
                    .on_press_maybe(
                        (!self.busy()).then(|| Message::Items(Event::Selected(item.id.clone()))),
                    )
                    .style(move |_, _| iced::widget::button::Style {
                        background: Some(
                            if selected {
                                super::mix(c.field, c.accent, 0.08)
                            } else {
                                c.field
                            }
                            .into(),
                        ),
                        text_color: c.foreground,
                        border: Border {
                            radius: 8.0.into(),
                            width: 1.0,
                            color: if selected { c.accent } else { c.line },
                        },
                        ..Default::default()
                    }),
                );
            }
            let results = column![
                self.label(format!("{} 个结果", editor.results.len())),
                super::smooth_scroll::scroll(
                    scrollable(list)
                        .height(280)
                        .spacing(8)
                        .id("custom-item-list"),
                    "custom-item-list",
                    self.reduced
                ),
                row![
                    self.action(
                        49,
                        "上一页",
                        (editor.page > 0 && !self.busy())
                            .then_some(Message::Items(Event::Page(false))),
                        false
                    ),
                    self.label(format!(
                        "{} / {}",
                        editor.page + 1,
                        editor.results.len().div_ceil(40).max(1)
                    )),
                    self.action(
                        50,
                        "下一页",
                        ((editor.page + 1) * 40 < editor.results.len() && !self.busy())
                            .then_some(Message::Items(Event::Page(true))),
                        false
                    )
                ]
                .spacing(8)
                .align_y(Alignment::Center)
            ]
            .spacing(8);
            let mut details = column![].spacing(10);
            if let Some(item) = editor
                .entries
                .iter()
                .find(|item| Some(&item.id) == editor.selected.as_ref())
            {
                details = details
                    .push(self.title(&item.chinese, 16))
                    .push(self.label(&item.english));
                if editor.entries.iter().any(|other| {
                    other.id != item.id
                        && (other.translation == item.translation || other.model == item.model)
                }) {
                    details = details
                        .push(self.label("此版本有物品共用名称或模型，修改对应设置可能一起生效。"));
                }
                if let Some(preview) = &editor.preview {
                    details = details.push(
                        container(
                            iced::widget::image(preview.clone())
                                .width(64)
                                .height(64)
                                .filter_method(iced::widget::image::FilterMethod::Nearest),
                        )
                        .padding(8)
                        .style(move |_| iced::widget::container::Style {
                            background: Some(c.field.into()),
                            ..Default::default()
                        }),
                    );
                }
                let field =
                    |placeholder: &'static str, value: String, event: fn(String) -> Event| {
                        self.item_input(placeholder, value, event)
                    };
                details = details
                    .push(self.label("自定义名字（留空沿用原名）"))
                    .push(field("物品名字", editor.name.clone(), Event::Name));
                if self.target == "1.7.10" {
                    details = details.push(
                        self.label("1.7.10 不读取资源包物品模型，支持改名和普通物品贴图描边。"),
                    );
                } else {
                    details = details
                        .push(self.label("显示大小 · 25%–400%，100% 保持原样"))
                        .push(
                            row![
                                column![
                                    self.label("物品栏 %"),
                                    field("100", editor.inventory.clone(), Event::Inventory)
                                ]
                                .spacing(4),
                                column![
                                    self.label("手持 %"),
                                    field("100", editor.held.clone(), Event::Held)
                                ]
                                .spacing(4),
                                column![
                                    self.label("掉落 %"),
                                    field("100", editor.dropped.clone(), Event::Dropped)
                                ]
                                .spacing(4),
                            ]
                            .spacing(8),
                        );
                }
                details = details.push(
                    checkbox(editor.border)
                        .label("贴图描边")
                        .text_size(13)
                        .size(17)
                        .on_toggle_maybe(
                            (!self.busy()).then_some(|value| Message::Items(Event::Border(value))),
                        ),
                );
                if editor.border {
                    details = details
                        .push(
                            row![
                                field("#000000", editor.color.clone(), Event::Color),
                                self.palette_button(
                                    96,
                                    super::color_picker::ColorTarget::ItemOutline,
                                    !self.busy()
                                ),
                                iced::widget::pick_list(
                                    [1u8, 2, 3, 4],
                                    Some(editor.width),
                                    |value| Message::Items(Event::Width(value))
                                )
                                .padding(9)
                            ]
                            .spacing(8),
                        )
                        .push(self.label("颜色填写十六进制，右侧选择 1–4 像素宽度。"));
                }
                details = details.push(
                    row![
                        self.action(
                            51,
                            "保存此物品",
                            (!self.busy()).then_some(Message::Items(Event::Save)),
                            true
                        ),
                        self.action(
                            52,
                            "恢复此物品",
                            (!self.busy()).then_some(Message::Items(Event::Reset)),
                            false
                        )
                    ]
                    .spacing(8),
                );
            }
            if self.width < 940.0 {
                content = content.push(results).push(details);
            } else {
                content = content.push(
                    row![
                        container(results).width(Length::FillPortion(1)),
                        container(details).width(Length::FillPortion(1))
                    ]
                    .spacing(20),
                );
            }
            if !editor.note.is_empty() {
                content = content.push(self.label(&editor.note));
            }
            content = content.push(self.action(
                53,
                "清除此版本的物品设置",
                (!self.busy() && count > 0).then_some(Message::Items(Event::Clear)),
                false,
            ));
            content=content.push(self.label("服务器命名可能覆盖语言名称；描边只改物品贴图，不改已放置的方块。特殊实体物品可能不支持贴图描边。"));
        }
        self.panel(content)
    }

    fn item_input(
        &self,
        placeholder: &'static str,
        value: String,
        event: fn(String) -> Event,
    ) -> iced::widget::TextInput<'_, Message> {
        let mut input = text_input(placeholder, &value)
            .size(13)
            .padding(9)
            .style(|_, status| self.input_style(status));
        if !self.busy() {
            input = input.on_input(move |value| Message::Items(event(value)));
        }
        input
    }
}
