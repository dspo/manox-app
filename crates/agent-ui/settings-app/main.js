// Steer Settings — a gpui-shell script application.
//
// The host (agent-ui/src/settings_shell.rs) opens this view as a native
// floating dialog on the window's ShellRoot. All data flows through the
// `settingsNative` HostModule: the script holds no filesystem or app state.
//
// Composition note: the upstream SettingItem value bindings are still
// "deferred" in the gpui-shell 0.8.1 catalog, so pages compose from the
// primitive components (v_flex/Switch/Button from "gpui-base") — this file is
// also a cost datum for migrating the full Rust panels.

import { div, View } from "gpui-kit";
import { Switch, h_flex, v_flex } from "gpui-base";
import { about, getLanguage, listMcp, setLanguage, setMcpEnabled } from "settingsNative";

const BG = "#ffffff";
const PANEL = "#fafafa";
const BORDER = "#e5e5e5";
const FG = "#171717";
const MUTED = "#737373";
const ACCENT = "#2563eb";

const PAGES = ["通用", "集成", "外部工具"];

export default class SettingsApp extends View {
  init() {
    this.english = getLanguage() === "en";
    this.mcp = JSON.parse(listMcp() || "[]");
    this.page = 0;
    this.notice = "";
  }

  render(cx) {
    const t = (zh, en) => (this.english ? en : zh);
    return v_flex()
      .size_full()
      .bg(BG)
      .child(this.header(t("设置", "Settings")))
      .child(
        h_flex()
          .flex_1()
          .min_h(0)
          .child(this.nav(t))
          .child(this.body(t)),
      )
      .children(this.notice ? [this.noticeBar(t)] : []);
  }

  header(title) {
    return h_flex()
      .h(44)
      .flex_none()
      .w_full()
      .px(16)
      .items_center()
      .border_b_1()
      .border_color(BORDER)
      .child(div().text_size(15.).font_weight(600).text_color(FG).child(title))
      .child(
        div()
          .ml(8)
          .text_size(11.)
          .text_color(MUTED)
          .child("gpui-shell script surface"),
      );
  }

  nav(t) {
    // Plain interactive divs, not Button: the base-layer Button accepts no
    // variant methods (`ghost` is a gpui-component catalog method — the run
    // error that hit the real app first), and a styled div needs nothing
    // beyond the universal element methods (id/on_click/when/hover).
    const names = this.english ? ["General", "Integrations", "External Tools"] : PAGES;
    let nav = v_flex().w(180).flex_none().p(8).gap(2).bg(PANEL);
    PAGES.forEach((_label, ix) => {
      const active = this.page === ix;
      nav = nav.child(
        div()
          .id(`nav-${ix}`)
          .w_full()
          .py(6)
          .px(8)
          .rounded(6.)
          .when(active, (el) => el.bg("#e8f0fe"))
          .hover((el) => el.bg(active ? "#dbeafe" : "#f0f0f0"))
          .on_click((_ev, cx2) => {
            this.page = ix;
            cx2.notify();
          })
          .child(
            div()
              .text_size(13.)
              .font_weight(active ? 600 : 400)
              .text_color(active ? ACCENT : FG)
              .child(names[ix]),
          ),
      );
    });
    return nav.child(
      div()
        .mt(8)
        .px(8)
        .text_size(11.)
        .text_color(MUTED)
        .child(t("脚本驱动 · 改动免重编译", "script-driven · hot on save")),
    );
  }

  body(t) {
    if (this.page === 0) return this.generalPage(t);
    if (this.page === 1) return this.integrationsPage(t);
    return this.placeholderPage(
      t("外部工具（ChatGPT.app / VS Code）——迁移评估中", "External tools (ChatGPT.app / VS Code) — pending migration"),
    );
  }

  generalPage(t) {
    return v_flex()
      .flex_1()
      .min_w(0)
      .p(20)
      .gap(16)
      .overflow_y_scroll()
      .child(this.sectionTitle(t("语言", "Language")))
      .child(
        this.row(
          t("英文界面", "English interface"),
          t("切换 app chrome 语言，保存后立即生效。", "Switch the app chrome language; applies immediately."),
          Switch.new("lang-switch")
            .checked(this.english)
            .on_change((checked, cx2) => {
              const token = checked ? "en" : "zh-CN";
              const reply = setLanguage(token);
              if (reply === "ok") {
                this.english = checked;
              } else {
                this.notice = reply;
              }
              cx2.notify();
            }),
        ),
      )
      .child(this.sectionTitle(t("关于", "About")))
      .child(this.aboutRows());
  }

  integrationsPage(t) {
    let page = v_flex()
      .flex_1()
      .min_w(0)
      .p(20)
      .gap(12)
      .overflow_y_scroll()
      .child(this.sectionTitle(t("MCP 服务器", "MCP servers")))
      .child(
        div()
          .text_size(12.)
          .text_color(MUTED)
          .child(t("开关即刻持久化，自下次启动生效。", "Toggles persist immediately; applied from the next launch.")),
      );
    if (this.mcp.length === 0) {
      page = page.child(
        div().py(12).text_size(13.).text_color(MUTED)
          .child(t("（未配置任何 MCP 服务器）", "(no MCP servers configured)")),
      );
    }
    for (const server of this.mcp) {
      const status = !server.enabled
        ? t("已禁用", "disabled")
        : server.tools > 0
          ? `${server.tools} ${t("个工具", "tools")}`
          : t("未连接", "not connected");
      page = page.child(
        this.row(
          server.name,
          status,
          Switch.new(`mcp-${server.name}`)
            .checked(server.enabled)
            .on_change((checked, cx2) => {
              const reply = setMcpEnabled(server.name, checked ? "true" : "false");
              if (reply === "ok") {
                server.enabled = checked;
              } else {
                this.notice = reply;
              }
              cx2.notify();
            }),
        ),
      );
    }
    return page;
  }

  placeholderPage(text) {
    return v_flex()
      .flex_1()
      .min_w(0)
      .p(20)
      .items_center()
      .justify_center()
      .child(div().text_size(13.).text_color(MUTED).child(text));
  }

  aboutRows() {
    let info;
    try {
      info = JSON.parse(about());
    } catch (e) {
      info = { app: "?", runtime: "?" };
    }
    return v_flex().gap(6).child(
      h_flex()
        .justify_between()
        .w_full()
        .child(div().text_size(13.).text_color(FG).child("Steer"))
        .child(div().text_size(13.).text_color(MUTED).child(info.app)),
    ).child(
      h_flex()
        .justify_between()
        .w_full()
        .child(div().text_size(13.).text_color(FG).child("Runtime"))
        .child(div().text_size(13.).text_color(MUTED).child(info.runtime)),
    );
  }

  sectionTitle(text) {
    return div()
      .text_size(12.)
      .font_weight(600)
      .text_color(MUTED)
      .child(text.toUpperCase());
  }

  row(title, description, control) {
    return h_flex()
      .w_full()
      .justify_between()
      .items_center()
      .p(12)
      .rounded(8.)
      .border_1()
      .border_color(BORDER)
      .child(
        v_flex()
          .gap(2)
          .child(div().text_size(13.).text_color(FG).child(title))
          .child(div().text_size(12.).text_color(MUTED).child(description)),
      )
      .child(control);
  }

  noticeBar(t) {
    return h_flex()
      .flex_none()
      .w_full()
      .h(32)
      .px(16)
      .items_center()
      .bg("#fef2f2")
      .child(div().text_size(12.).text_color("#b91c1c").child(this.notice));
  }
}
