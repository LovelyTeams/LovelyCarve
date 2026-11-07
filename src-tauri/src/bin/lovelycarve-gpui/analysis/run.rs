//! 快速分析: run plugin analyzers on this evidence or one of its detected
//! systems. Jobs run in the plugin scheduler; the task center follows them.
use std::collections::BTreeSet;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        radio::RadioGroup,
        scroll::ScrollableElement as _,
        spinner::Spinner,
        v_flex, ActiveTheme as _, Disableable as _, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    Context, EventEmitter, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window,
};
use lovelycarve_lib::{
    commands::extensions::task_scheduler as scheduler_commands,
    core::{
        analysis::identify::systems::DetectedSystem,
        extensions::{plugin::AnalyzerScope, task_scheduler::SchedFunction},
    },
    host::State,
};

use crate::{backend::Backend, ui};

/// The dialog started this many jobs.
#[derive(Clone, Debug)]
pub struct AnalysisStarted(pub usize);

enum Load<T> {
    Loading,
    Loaded(T),
    Failed(SharedString),
}

pub struct QuickAnalysis {
    image_id: String,
    functions: Load<Vec<SchedFunction>>,
    systems: Load<Vec<DetectedSystem>>,
    system: usize,
    selected: BTreeSet<String>,
    submitting: bool,
    error: Option<SharedString>,
}

impl EventEmitter<AnalysisStarted> for QuickAnalysis {}

impl QuickAnalysis {
    pub fn new(image_id: String, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let functions = backend.run(|_, state| async move {
            scheduler_commands::sched_task_functions(State::new(&state)).await
        });
        let system_image = image_id.clone();
        let systems = backend.run(move |_, state| async move {
            scheduler_commands::sched_systems_list(State::new(&state), system_image).await
        });
        cx.spawn(async move |this, cx| {
            let functions = functions.await;
            this.update(cx, |this, cx| {
                this.functions = match functions {
                    Ok(functions) => Load::Loaded(functions),
                    Err(error) => Load::Failed(error.message),
                };
                cx.notify();
            })
            .ok();
            let systems = systems.await;
            this.update(cx, |this, cx| {
                this.systems = match systems {
                    Ok(systems) => Load::Loaded(systems),
                    Err(error) => Load::Failed(error.message),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            image_id,
            functions: Load::Loading,
            systems: Load::Loading,
            system: 0,
            selected: BTreeSet::new(),
            submitting: false,
            error: None,
        }
    }

    fn current_system(&self) -> Option<&DetectedSystem> {
        match &self.systems {
            Load::Loaded(systems) => systems.get(self.system),
            _ => None,
        }
    }

    /// Analyzers that can run on the evidence or the chosen system.
    fn runnable(&self) -> Vec<SchedFunction> {
        let Load::Loaded(functions) = &self.functions else {
            return Vec::new();
        };
        let family = self.current_system().map(|system| system.family.clone());
        functions
            .iter()
            .filter(|function| !function.internal)
            .filter(|function| match function.scope {
                AnalyzerScope::Evidence => true,
                AnalyzerScope::System => family.as_ref().is_some_and(|family| {
                    function.platforms.is_empty()
                        || function.platforms.iter().any(|platform| platform == family)
                }),
                AnalyzerScope::Files => false,
            })
            .cloned()
            .collect()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let runnable = self.runnable();
        let chosen: Vec<SchedFunction> = runnable
            .into_iter()
            .filter(|function| self.selected.contains(&function.function))
            .collect();
        if chosen.is_empty() {
            self.error = Some("至少选择一个分析器".into());
            cx.notify();
            return;
        }
        self.submitting = true;
        self.error = None;
        cx.notify();
        let backend = Backend::global(cx);
        let image_id = self.image_id.clone();
        let system_id = self.current_system().map(|system| system.id.clone());
        cx.spawn(async move |this, cx| {
            let mut created = 0;
            let mut failure = None;
            for function in chosen {
                let image_id = image_id.clone();
                let system = matches!(function.scope, AnalyzerScope::System)
                    .then(|| system_id.clone())
                    .flatten();
                let name = function.function.clone();
                let result = backend
                    .run(move |_, state| async move {
                        scheduler_commands::sched_task_create(
                            State::new(&state),
                            name,
                            image_id,
                            None,
                            None,
                            system,
                            None,
                        )
                        .await
                    })
                    .await;
                match result {
                    Ok(_) => created += 1,
                    Err(error) => {
                        failure = Some(format!("{}：{}", function.display_name, error.message))
                    }
                }
            }
            this.update(cx, |this, cx| {
                this.submitting = false;
                this.error = failure.map(SharedString::from);
                if created > 0 {
                    cx.emit(AnalysisStarted(created));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for QuickAnalysis {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = matches!(self.functions, Load::Loading);
        let runnable = self.runnable();
        let system_labels: Vec<SharedString> = match &self.systems {
            Load::Loaded(systems) => systems
                .iter()
                .map(|system| SharedString::from(system.label.clone()))
                .collect(),
            _ => Vec::new(),
        };
        v_flex()
            .w_full()
            .gap_3()
            .child(ui::section_title("分析对象", cx))
            .child(match &self.systems {
                Load::Loading => h_flex()
                    .gap_2()
                    .text_sm()
                    .child(Spinner::new().small())
                    .child("正在识别系统…")
                    .into_any_element(),
                Load::Failed(error) => {
                    ui::error_text(format!("识别系统失败：{error}"), cx).into_any_element()
                }
                Load::Loaded(systems) if systems.is_empty() => div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("未识别到操作系统；只列出检材级分析器。")
                    .into_any_element(),
                Load::Loaded(_) => RadioGroup::vertical("analysis-system")
                    .children(system_labels)
                    .selected_index(Some(self.system))
                    .on_change(cx.listener(|this, index: &usize, _, cx| {
                        this.system = *index;
                        this.selected.clear();
                        cx.notify();
                    }))
                    .into_any_element(),
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::section_title(
                        format!("分析器 · {}", runnable.len()),
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new("select-analyzers")
                            .xsmall()
                            .ghost()
                            .label("全选")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected = this
                                    .runnable()
                                    .into_iter()
                                    .map(|function| function.function)
                                    .collect();
                                cx.notify();
                            })),
                    ),
            )
            .child(match &self.functions {
                Load::Loading => h_flex()
                    .gap_2()
                    .text_sm()
                    .child(Spinner::new().small())
                    .child("正在加载分析插件…")
                    .into_any_element(),
                Load::Failed(error) => ui::error_text(error.clone(), cx).into_any_element(),
                Load::Loaded(_) if runnable.is_empty() => div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("暂无可用的分析插件；安装提供系统或检材分析的插件后会显示在这里。")
                    .into_any_element(),
                Load::Loaded(_) => v_flex()
                    .max_h(gpui_kit::px(300.))
                    .gap_2()
                    .children(runnable.iter().map(|function| {
                        let name = function.function.clone();
                        Checkbox::new(SharedString::from(format!(
                            "analyzer-{}",
                            function.function
                        )))
                        .label(format!(
                            "{}（{}）",
                            function.display_name, function.plugin_name
                        ))
                        .checked(self.selected.contains(&function.function))
                        .on_click(cx.listener(
                            move |this, checked: &bool, _, cx| {
                                if *checked {
                                    this.selected.insert(name.clone());
                                } else {
                                    this.selected.remove(&name);
                                }
                                cx.notify();
                            },
                        ))
                    }))
                    .overflow_y_scrollbar()
                    .into_any_element(),
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(ui::error_text(error, cx))
            })
            .child(
                h_flex().justify_end().gap_2().child(
                    Button::new("start-analysis")
                        .primary()
                        .label(if self.submitting {
                            "正在提交…"
                        } else {
                            "开始分析"
                        })
                        .disabled(loading || self.submitting || self.selected.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                ),
            )
    }
}
