use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, VK_CONTROL, KEYEVENTF_KEYUP, VK_V,
};

/// 模拟 Ctrl+V 按键，将剪贴板内容粘贴到当前前台窗口
pub fn simulate_ctrl_v() {
    unsafe {
        // 按下 Ctrl
        keybd_event(VK_CONTROL as u8, 0, 0, 0);
        // 按下 V
        keybd_event(VK_V as u8, 0, 0, 0);
        // 释放 V
        keybd_event(VK_V as u8, 0, KEYEVENTF_KEYUP, 0);
        // 释放 Ctrl
        keybd_event(VK_CONTROL as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}
