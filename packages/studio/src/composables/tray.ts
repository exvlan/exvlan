import { Menu, MenuItem, PredefinedMenuItem } from "@tauri-apps/api/menu";
import { TrayIcon } from "@tauri-apps/api/tray";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type } from "@tauri-apps/plugin-os";
import pkg from "~/../package.json";

const DEFAULT_TRAY_NAME = "main";

function isMobilePlatform() {
  const t = type();
  return t === "android" || t === "ios";
}

async function toggleVisibility() {
  if (await getCurrentWindow().isVisible()) {
    await getCurrentWindow().hide();
  } else {
    await getCurrentWindow().show();
    await getCurrentWindow().setFocus();
  }
}

export async function useTray(init: boolean = false) {
  // 移动端没有托盘/原生菜单：直接返回，避免调用 `plugin:menu|*` 触发
  // `plugin menu not found`（Android pre.10 包里出现过 6 次）。
  if (isMobilePlatform()) {
    return null;
  }
  let tray;
  try {
    tray = await TrayIcon.getById(DEFAULT_TRAY_NAME);
    if (!tray) {
      tray = await TrayIcon.new({
        tooltip: `Exvlan\n${pkg.version}`,
        title: `Exvlan\n${pkg.version}`,
        id: DEFAULT_TRAY_NAME,
        menu: await Menu.new({
          id: "main",
          items: await generateMenuItem(),
        }),
        action: async () => {
          void toggleVisibility();
        },
      });
    }
  } catch (error) {
    console.warn("Error while creating tray icon:", error);
    return null;
  }

  if (init) {
    void tray.setTooltip(`Exvlan\n${pkg.version}`);
    void tray.setMenuOnLeftClick(false);
    void tray.setMenu(
      await Menu.new({
        id: "main",
        items: await generateMenuItem(),
      }),
    );
  }

  return tray;
}

export async function generateMenuItem() {
  if (isMobilePlatform()) {
    return [];
  }
  return [
    await MenuItemShow("Show / Hide"),
    await PredefinedMenuItem.new({ item: "Separator" }),
    await MenuItemExit("Exit"),
  ];
}

export async function MenuItemExit(text: string) {
  return await PredefinedMenuItem.new({
    text,
    item: "Quit",
  });
}

export async function MenuItemShow(text: string) {
  return await MenuItem.new({
    id: "show",
    text,
    action: async () => {
      await toggleVisibility();
    },
  });
}

export async function setTrayMenu(items?: (MenuItem | PredefinedMenuItem)[]) {
  const tray = await useTray();
  if (!tray) return;
  const menu = await Menu.new({
    id: "main",
    items: items || (await generateMenuItem()),
  });
  void tray.setMenu(menu);
}

export async function setTrayRunState(isRunning: boolean = false) {
  const tray = await useTray();
  if (!tray) return;
  void tray.setIcon(isRunning ? "icons/icon-inactive.ico" : "icons/icon.ico");
}

export async function setTrayTooltip(tooltip: string) {
  if (tooltip) {
    const tray = await useTray();
    if (!tray) return;
    void tray.setTooltip(`Exvlan\n${pkg.version}\n${tooltip}`);
    void tray.setTitle(`Exvlan\n${pkg.version}\n${tooltip}`);
  }
}
