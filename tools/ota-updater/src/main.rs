#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::ffi::CString;
use std::fs;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use eframe::egui;
use eframe::egui::{FontData, FontDefinitions, FontFamily};
use hidapi::{HidApi, HidDevice};
use reqwest::blocking::Client;
use reqwest::Url;
use sha2::{Digest, Sha256};

const DEFAULT_REPO: &str = "mainlxl/ds5dongle-bl618-ota";
const REPORT_ID_CONFIG: u8 = 0xf6;
const REPORT_ID_VERSION: u8 = 0xf8;
const REPORT_ID_STATUS: u8 = 0xf9;
const OTA_CMD_START: u8 = 0x10;
const OTA_CMD_DATA: u8 = 0x11;
const OTA_CMD_FINISH: u8 = 0x12;
const OTA_CMD_ABORT: u8 = 0x13;
const OTA_CHUNK_SIZE: usize = 59;
const SONY_VID: u16 = 0x054c;
const SUPPORTED_PIDS: [u16; 2] = [0x0ce6, 0x0df2];

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([880.0, 640.0]),
        ..Default::default()
    };

    eframe::run_native(
        "DS5Dongle OTA Updater",
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            Ok(Box::new(OtaApp::default()))
        }),
    )
}

fn configure_fonts(ctx: &egui::Context) {
    let Some((name, font_data)) = load_system_cjk_font() else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(name.clone(), font_data);
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, name.clone());
    }
    ctx.set_fonts(fonts);
}

fn load_system_cjk_font() -> Option<(String, FontData)> {
    let candidates: &[(&str, u32)] = &[
        ("/System/Library/Fonts/PingFang.ttc", 0),
        ("/System/Library/Fonts/LanguageSupport/PingFang.ttc", 0),
        ("/System/Library/Fonts/STHeiti Light.ttc", 0),
        ("/System/Library/Fonts/Supplemental/Songti.ttc", 0),
        ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
        ("/Library/Fonts/Arial Unicode.ttf", 0),
        ("C:\\Windows\\Fonts\\msyh.ttc", 0),
        ("C:\\Windows\\Fonts\\simhei.ttf", 0),
        ("C:\\Windows\\Fonts\\simsun.ttc", 0),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
        (
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            0,
        ),
        ("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc", 2),
        (
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            2,
        ),
        ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
        ("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc", 0),
    ];

    for (path, index) in candidates {
        if let Ok(bytes) = fs::read(path) {
            let mut data = FontData::from_owned(bytes);
            data.index = *index;
            return Some((format!("system-cjk-{index}"), data));
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Speed {
    Fs,
    Hs,
}

impl Speed {
    fn as_str(self) -> &'static str {
        match self {
            Speed::Fs => "fs",
            Speed::Hs => "hs",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Speed::Fs => "Full-Speed",
            Speed::Hs => "High-Speed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogLevel {
    Info,
    Warn,
    Error,
    Success,
}

#[derive(Clone, Debug)]
struct LogLine {
    level: LogLevel,
    text: String,
}

#[derive(Clone, Debug)]
struct ReleaseInfo {
    tag: String,
    ota_name: String,
    ota_url: String,
    checksum_name: String,
    checksum_url: String,
}

#[derive(Clone, Debug)]
struct DownloadedPackage {
    name: String,
    bytes: Vec<u8>,
    sha256: String,
}

#[derive(Clone, Debug, Default)]
struct OtaStatus {
    status: u8,
    label: String,
    received: u32,
    total: u32,
    error_detail: u8,
    error_label: String,
    error_address: u32,
    payload_flushed: u32,
    raw_hex: String,
}

enum WorkerMsg {
    Busy(bool),
    Log(LogLevel, String),
    Progress(f32, String),
    Version(String),
    Latest(ReleaseInfo),
    Package(DownloadedPackage),
}

struct OtaApp {
    repo: String,
    speed: Speed,
    local_path: String,
    busy: bool,
    progress: f32,
    progress_text: String,
    version: String,
    latest: Option<ReleaseInfo>,
    package: Option<DownloadedPackage>,
    auto_start_after_download: bool,
    logs: Vec<LogLine>,
    tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,
}

impl Default for OtaApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            repo: DEFAULT_REPO.to_string(),
            speed: Speed::Hs,
            local_path: String::new(),
            busy: false,
            progress: 0.0,
            progress_text: "等待操作".to_string(),
            version: String::new(),
            latest: None,
            package: None,
            auto_start_after_download: false,
            logs: vec![LogLine {
                level: LogLevel::Info,
                text: "程序已启动。先连接设备读取版本，再检查最新 Release。".to_string(),
            }],
            tx,
            rx,
        }
    }
}

impl OtaApp {
    fn poll_worker(&mut self) {
        let mut became_idle = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                WorkerMsg::Busy(value) => {
                    self.busy = value;
                    if !value {
                        became_idle = true;
                    }
                }
                WorkerMsg::Log(level, text) => self.push_log(level, text),
                WorkerMsg::Progress(value, text) => {
                    self.progress = value.clamp(0.0, 1.0);
                    self.progress_text = text;
                }
                WorkerMsg::Version(version) => {
                    let detected_speed = infer_speed_from_version(&version);
                    if detected_speed != self.speed {
                        self.speed = detected_speed;
                        self.push_log(
                            LogLevel::Info,
                            format!("已根据固件版本自动选择 {}", detected_speed.label()),
                        );
                    }
                    self.version = version;
                }
                WorkerMsg::Latest(latest) => {
                    self.latest = Some(latest);
                    self.package = None;
                }
                WorkerMsg::Package(package) => self.package = Some(package),
            }
        }

        if became_idle && self.auto_start_after_download && !self.busy {
            self.auto_start_after_download = false;
            if self.package.is_some() {
                self.push_log(LogLevel::Info, "下载完成，自动开始 OTA");
                self.start_ota();
            } else {
                self.push_log(LogLevel::Error, "自动下载 OTA 包失败，已停止 OTA");
            }
        }
    }

    fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        self.logs.push(LogLine {
            level,
            text: format!("[{}] {}", current_time(), text.into()),
        });
        if self.logs.len() > 500 {
            self.logs.drain(0..self.logs.len() - 500);
        }
    }

    fn run_task<F>(&mut self, task: F)
    where
        F: FnOnce(Sender<WorkerMsg>) -> Result<()> + Send + 'static,
    {
        if self.busy {
            return;
        }
        self.busy = true;
        self.progress = 0.0;
        self.progress_text = "执行中".to_string();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(WorkerMsg::Busy(true));
            if let Err(err) = task(tx.clone()) {
                send_log(&tx, LogLevel::Error, format!("{err:#}"));
            }
            let _ = tx.send(WorkerMsg::Busy(false));
        });
    }

    fn open_device_for_task(&mut self) -> Option<DeviceSession> {
        if self.busy {
            return None;
        }

        match open_device() {
            Ok(session) => Some(session),
            Err(err) => {
                self.push_log(LogLevel::Error, format!("{err:#}"));
                self.progress = 0.0;
                self.progress_text = "连接失败".to_string();
                None
            }
        }
    }

    fn read_version(&mut self) {
        let Some(session) = self.open_device_for_task() else {
            return;
        };
        self.run_task(move |tx| {
            send_log(&tx, LogLevel::Success, format!("已连接：{}", session.label));
            let version = read_firmware_version(&session.device)?;
            let _ = tx.send(WorkerMsg::Version(version.clone()));
            send_log(&tx, LogLevel::Info, format!("固件版本：{version}"));
            let _ = tx.send(WorkerMsg::Progress(1.0, "读取完成".to_string()));
            Ok(())
        });
    }

    fn check_latest(&mut self) {
        let repo = self.repo.clone();
        let speed = self.speed;
        self.run_task(move |tx| {
            let latest = resolve_latest_release(&repo, speed)?;
            send_log(&tx, LogLevel::Info, format!("最新 Release：{}", latest.tag));
            send_log(&tx, LogLevel::Info, format!("OTA 包：{}", latest.ota_name));
            let _ = tx.send(WorkerMsg::Latest(latest));
            let _ = tx.send(WorkerMsg::Progress(1.0, "已获取最新版本".to_string()));
            Ok(())
        });
    }

    fn download_latest(&mut self) {
        let repo = self.repo.clone();
        let speed = self.speed;
        self.run_task(move |tx| {
            let latest = resolve_latest_release(&repo, speed)?;
            let package = download_release_package(&latest, &tx)?;
            send_log(
                &tx,
                LogLevel::Success,
                format!("下载完成：{}，SHA256 {}", package.name, package.sha256),
            );
            let _ = tx.send(WorkerMsg::Latest(latest));
            let _ = tx.send(WorkerMsg::Package(package));
            let _ = tx.send(WorkerMsg::Progress(1.0, "下载完成".to_string()));
            Ok(())
        });
    }

    fn start_ota(&mut self) {
        let local_path = self.local_path.trim().to_string();
        if local_path.is_empty() && self.package.is_none() {
            self.auto_start_after_download = true;
            self.push_log(
                LogLevel::Info,
                "未找到已下载 OTA 包，自动下载最新 Release OTA 包",
            );
            self.download_latest();
            return;
        }

        let package = if local_path.is_empty() {
            match self.package.clone() {
                Some(package) => package,
                None => {
                    self.push_log(LogLevel::Error, "还没有 OTA 包，自动下载未完成");
                    return;
                }
            }
        } else {
            match read_local_package(&local_path) {
                Ok(package) => {
                    self.push_log(LogLevel::Info, format!("使用本地 OTA 包：{}", package.name));
                    self.push_log(
                        LogLevel::Info,
                        format!("本地 OTA 包 SHA256：{}", package.sha256),
                    );
                    package
                }
                Err(err) => {
                    self.push_log(LogLevel::Error, format!("{err:#}"));
                    return;
                }
            }
        };

        if self.busy {
            return;
        }
        self.busy = true;
        self.progress = 0.0;
        self.progress_text = "探测 OTA 接口".to_string();
        let package_size = package.bytes.len();
        let session =
            match open_ota_device(package_size, |level, message| self.push_log(level, message)) {
                Ok(session) => session,
                Err(err) => {
                    self.busy = false;
                    self.progress_text = "OTA 接口探测失败".to_string();
                    self.push_log(LogLevel::Error, format!("{err:#}"));
                    return;
                }
            };
        self.busy = false;

        self.run_task(move |tx| perform_ota(session, package, &tx));
    }

    fn abort_ota(&mut self) {
        let Some(session) = self.open_device_for_task() else {
            return;
        };
        self.run_task(move |tx| {
            send_feature_command(&session.device, &[OTA_CMD_ABORT], session.report_mode)?;
            send_log(&tx, LogLevel::Warn, "已发送 OTA 中止命令");
            let _ = tx.send(WorkerMsg::Progress(0.0, "已中止".to_string()));
            Ok(())
        });
    }
}

impl eframe::App for OtaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        ctx.request_repaint_after(Duration::from_millis(100));

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("DS5Dongle OTA 更新工具");
                ui.separator();
                ui.label("GitHub Release 下载 + HID OTA 写入");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("仓库");
                ui.add_sized([360.0, 24.0], egui::TextEdit::singleline(&mut self.repo));
                ui.radio_value(&mut self.speed, Speed::Hs, "High-Speed");
                ui.radio_value(&mut self.speed, Speed::Fs, "Full-Speed");
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("连接并读取版本"))
                    .clicked()
                {
                    self.read_version();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("检查最新 Release"))
                    .clicked()
                {
                    self.check_latest();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("下载最新 OTA 包"))
                    .clicked()
                {
                    self.download_latest();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("开始 OTA 更新"))
                    .clicked()
                {
                    self.start_ota();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("发送中止"))
                    .clicked()
                {
                    self.abort_ota();
                }
            });

            ui.add_space(10.0);
            egui::Grid::new("status_grid")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("当前固件版本");
                    ui.monospace(if self.version.is_empty() {
                        "-"
                    } else {
                        &self.version
                    });
                    ui.end_row();

                    ui.label("最新 Release");
                    ui.monospace(self.latest.as_ref().map(|v| v.tag.as_str()).unwrap_or("-"));
                    ui.end_row();

                    ui.label("下载 OTA 包");
                    ui.monospace(
                        self.package
                            .as_ref()
                            .map(|v| v.name.as_str())
                            .unwrap_or("-"),
                    );
                    ui.end_row();

                    ui.label("SHA256");
                    ui.monospace(
                        self.package
                            .as_ref()
                            .map(|v| v.sha256.as_str())
                            .unwrap_or("-"),
                    );
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label("本地 .bin.ota");
                ui.add_sized(
                    [620.0, 24.0],
                    egui::TextEdit::singleline(&mut self.local_path)
                        .hint_text("可选：填写本地 OTA 包路径，留空则使用已下载的 Release 包"),
                );
            });

            ui.add_space(10.0);
            ui.add(egui::ProgressBar::new(self.progress).text(&self.progress_text));

            ui.add_space(10.0);
            ui.separator();
            ui.label("日志");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for line in &self.logs {
                        let color = match line.level {
                            LogLevel::Info => ui.visuals().text_color(),
                            LogLevel::Warn => egui::Color32::from_rgb(210, 140, 20),
                            LogLevel::Error => egui::Color32::from_rgb(220, 60, 60),
                            LogLevel::Success => egui::Color32::from_rgb(40, 150, 80),
                        };
                        ui.colored_label(color, &line.text);
                    }
                });
        });
    }
}

struct DeviceSession {
    device: HidDevice,
    label: String,
    report_mode: FeatureReportMode,
}

#[derive(Clone, Debug)]
struct DeviceCandidate {
    path: CString,
    label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeatureReportMode {
    Short,
    Padded64,
}

impl FeatureReportMode {
    fn label(self) -> &'static str {
        match self {
            FeatureReportMode::Short => "短包",
            FeatureReportMode::Padded64 => "64字节补齐",
        }
    }
}

fn open_device() -> Result<DeviceSession> {
    let api = HidApi::new().context("初始化 HID 失败")?;
    let candidates = collect_device_candidates(&api);
    if candidates.is_empty() {
        bail!(
            "没有找到 DS5Dongle 设备。请确认设备已插入、系统允许 HID 访问，并且固件支持 OTA Feature Report。"
        );
    }

    let mut last_error = None;
    for candidate in &candidates {
        match api.open_path(&candidate.path) {
            Ok(device) => match read_firmware_version(&device) {
                Ok(version) if version.contains("LCT616-DS5") || version.contains("DS5") => {
                    return Ok(DeviceSession {
                        device,
                        label: candidate.label.clone(),
                        report_mode: FeatureReportMode::Short,
                    });
                }
                Ok(_) => {
                    last_error = Some(format!(
                        "{} 可打开，但未读到 DS5Dongle 版本",
                        candidate.label
                    ));
                }
                Err(err) => {
                    last_error = Some(format!("{} 读取版本失败：{err:#}", candidate.label));
                }
            },
            Err(err) => {
                last_error = Some(format!("{} 打开失败：{err:#}", candidate.label));
            }
        }
    }

    let detail = last_error.unwrap_or_else(|| "没有可用 HID path".to_string());
    Err(anyhow!(
        "没有找到可读取 DS5Dongle 版本的 HID 接口。最后错误：{detail}"
    ))
}

fn open_ota_device<F>(package_size: usize, mut log: F) -> Result<DeviceSession>
where
    F: FnMut(LogLevel, String),
{
    let api = HidApi::new().context("初始化 HID 失败")?;
    let candidates = collect_device_candidates(&api);
    if candidates.is_empty() {
        bail!(
            "没有找到 DS5Dongle 设备。请确认设备已插入、系统允许 HID 访问，并且固件支持 OTA Feature Report。"
        );
    }

    log(
        LogLevel::Info,
        format!(
            "发现 {} 个匹配 HID 接口，开始探测 OTA 写入接口",
            candidates.len()
        ),
    );
    for candidate in &candidates {
        log(LogLevel::Info, format!("探测接口：{}", candidate.label));
        for mode in [FeatureReportMode::Short, FeatureReportMode::Padded64] {
            match api.open_path(&candidate.path) {
                Ok(device) => {
                    let version = read_firmware_version(&device)
                        .unwrap_or_else(|_| "无法读取版本".to_string());
                    log(
                        LogLevel::Info,
                        format!("尝试 {} 发送 START，版本：{}", mode.label(), version),
                    );
                    let _ = send_feature_command(&device, &[OTA_CMD_ABORT], mode);
                    let start = make_start_command(package_size)?;
                    if let Err(err) = send_feature_command(&device, &start, mode) {
                        log(
                            LogLevel::Warn,
                            format!("{} START 发送失败：{err:#}", mode.label()),
                        );
                        continue;
                    }
                    match wait_for_receive_started(&device, package_size) {
                        Ok(status) => {
                            log(
                                LogLevel::Success,
                                format!(
                                    "OTA 接口已确认：{}，发送方式：{}，设备总大小 {}",
                                    candidate.label,
                                    mode.label(),
                                    status.total
                                ),
                            );
                            return Ok(DeviceSession {
                                device,
                                label: candidate.label.clone(),
                                report_mode: mode,
                            });
                        }
                        Err(err) => {
                            log(
                                LogLevel::Warn,
                                format!("{} 未进入 OTA 接收：{err:#}", mode.label()),
                            );
                            let _ = send_feature_command(&device, &[OTA_CMD_ABORT], mode);
                        }
                    }
                }
                Err(err) => {
                    log(LogLevel::Warn, format!("打开接口失败：{err:#}"));
                }
            }
        }
    }

    bail!("所有匹配 HID 接口都没有进入 OTA 接收状态，请把本次探测日志发我继续定位")
}

fn collect_device_candidates(api: &HidApi) -> Vec<DeviceCandidate> {
    api.device_list()
        .filter(|device| {
            device.vendor_id() == SONY_VID && SUPPORTED_PIDS.contains(&device.product_id())
        })
        .map(|info| {
            let product = info.product_string().unwrap_or("DS5Dongle");
            let serial = info.serial_number().unwrap_or("-");
            let path = info.path().to_owned();
            let label = format!(
                "{} VID:{:04x} PID:{:04x} iface:{} usage:{:04x}:{:04x} SN:{} path:{}",
                product,
                info.vendor_id(),
                info.product_id(),
                info.interface_number(),
                info.usage_page(),
                info.usage(),
                serial,
                path.to_string_lossy()
            );
            DeviceCandidate { path, label }
        })
        .collect()
}

fn read_local_package(local_path: &str) -> Result<DownloadedPackage> {
    let bytes =
        fs::read(local_path).with_context(|| format!("读取本地 OTA 包失败：{local_path}"))?;
    let name = local_path
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("local.bin.ota")
        .to_string();
    let sha256 = sha256_hex(&bytes);
    Ok(DownloadedPackage {
        name,
        bytes,
        sha256,
    })
}

fn read_firmware_version(device: &HidDevice) -> Result<String> {
    let mut buf = [0u8; 64];
    buf[0] = REPORT_ID_VERSION;
    let len = device
        .get_feature_report(&mut buf)
        .context("读取固件版本失败")?;
    let payload = report_payload(&buf[..len], REPORT_ID_VERSION);
    let end = payload
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(payload.len());
    Ok(String::from_utf8_lossy(&payload[..end]).trim().to_string())
}

fn read_ota_status(device: &HidDevice) -> Result<OtaStatus> {
    let mut buf = [0u8; 64];
    buf[0] = REPORT_ID_STATUS;
    let len = device
        .get_feature_report(&mut buf)
        .context("读取 OTA 状态失败")?;
    let payload = report_payload(&buf[..len], REPORT_ID_STATUS);
    let status = *payload.get(2).unwrap_or(&0);
    let received = read_u32_le(payload, 3).unwrap_or(0);
    let total = read_u32_le(payload, 7).unwrap_or(0);
    let error_detail = *payload.get(13).unwrap_or(&0);
    let error_address = read_u32_le(payload, 14).unwrap_or(0);
    let payload_flushed = read_u32_le(payload, 18).unwrap_or(0);
    Ok(OtaStatus {
        status,
        label: status_label(status).to_string(),
        received,
        total,
        error_detail,
        error_label: error_detail_label(error_detail).to_string(),
        error_address,
        payload_flushed,
        raw_hex: hex::encode(&buf[..len]),
    })
}

fn report_payload<'a>(buf: &'a [u8], report_id: u8) -> &'a [u8] {
    if buf.first() == Some(&report_id) {
        &buf[1..]
    } else {
        buf
    }
}

fn read_u32_le(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn send_feature_command(device: &HidDevice, command: &[u8], mode: FeatureReportMode) -> Result<()> {
    let report = make_feature_report(command, mode)?;
    device
        .send_feature_report(&report)
        .context("发送 OTA HID 命令失败")?;
    Ok(())
}

fn make_feature_report(command: &[u8], mode: FeatureReportMode) -> Result<Vec<u8>> {
    if command.len() > 63 {
        bail!("OTA 命令过长：{} bytes", command.len());
    }
    let len = match mode {
        FeatureReportMode::Short => 1 + command.len(),
        FeatureReportMode::Padded64 => 64,
    };
    let mut report = vec![0u8; len];
    report[0] = REPORT_ID_CONFIG;
    report[1..1 + command.len()].copy_from_slice(command);
    Ok(report)
}

fn make_start_command(package_size: usize) -> Result<[u8; 5]> {
    let size = u32::try_from(package_size).context("OTA 包超过 4GB，设备不支持")?;
    let mut command = [0u8; 5];
    command[0] = OTA_CMD_START;
    command[1..5].copy_from_slice(&size.to_le_bytes());
    Ok(command)
}

fn make_data_command(seq: u16, chunk: &[u8]) -> Result<Vec<u8>> {
    if chunk.len() > OTA_CHUNK_SIZE {
        bail!("OTA chunk 超过限制：{} bytes", chunk.len());
    }
    let mut command = Vec::with_capacity(4 + chunk.len());
    command.push(OTA_CMD_DATA);
    command.extend_from_slice(&seq.to_le_bytes());
    command.push(u8::try_from(chunk.len()).context("OTA chunk 长度超过 u8")?);
    command.extend_from_slice(chunk);
    Ok(command)
}

fn perform_ota(
    session: DeviceSession,
    package: DownloadedPackage,
    tx: &Sender<WorkerMsg>,
) -> Result<()> {
    send_log(tx, LogLevel::Success, format!("已连接：{}", session.label));
    send_log(tx, LogLevel::Info, format!("准备写入：{}", package.name));
    send_log(
        tx,
        LogLevel::Info,
        format!("Feature Report 发送方式：{}", session.report_mode.label()),
    );

    let initial = read_ota_status(&session.device)?;
    ensure_status_ok(&initial)?;
    ensure_transfer_status(&initial, package.bytes.len())?;
    send_log(
        tx,
        LogLevel::Info,
        format!(
            "设备 OTA 状态：{}，总大小 {} bytes",
            initial.label,
            package.bytes.len()
        ),
    );

    let total_chunks = package.bytes.chunks(OTA_CHUNK_SIZE).len();
    for (index, chunk) in package.bytes.chunks(OTA_CHUNK_SIZE).enumerate() {
        let seq = u16::try_from(index).context("OTA chunk 数量超过协议限制")?;
        let command = make_data_command(seq, chunk)?;
        send_feature_command(&session.device, &command, session.report_mode)?;

        if index % 16 == 0 || index + 1 == total_chunks {
            let status = read_ota_status(&session.device)?;
            ensure_status_ok(&status)?;
            ensure_transfer_status(&status, package.bytes.len())?;
            let progress = ((index + 1) as f32 / total_chunks as f32) * 0.92;
            let _ = tx.send(WorkerMsg::Progress(
                progress,
                format!("发送中：{}/{} chunks", index + 1, total_chunks),
            ));
            send_log(
                tx,
                LogLevel::Info,
                format!(
                    "写入进度：{} / {} bytes，已落盘 {} bytes",
                    status.received, status.total, status.payload_flushed
                ),
            );
        }
        thread::sleep(Duration::from_millis(2));
    }

    send_feature_command(&session.device, &[OTA_CMD_FINISH], session.report_mode)?;
    send_log(
        tx,
        LogLevel::Info,
        "OTA 包已发送完成，等待设备校验并切换分区",
    );
    let _ = tx.send(WorkerMsg::Progress(0.95, "等待设备校验".to_string()));

    let deadline = Instant::now() + Duration::from_secs(180);
    let mut idle_reads = 0u8;
    while Instant::now() < deadline {
        match read_ota_status(&session.device) {
            Ok(status) => {
                ensure_status_ok(&status)?;
                if status.status == 3 {
                    send_log(
                        tx,
                        LogLevel::Success,
                        "OTA 完成；设备会重新枚举，请重新连接后读取版本",
                    );
                    let _ = tx.send(WorkerMsg::Progress(1.0, "OTA 完成".to_string()));
                    return Ok(());
                }
                if status.status == 0 && status.total == 0 {
                    idle_reads = idle_reads.saturating_add(1);
                    if idle_reads >= 10 {
                        bail!("设备没有进入 OTA 校验状态，FINISH 可能没有被固件接收");
                    }
                } else {
                    idle_reads = 0;
                }
                let label = format!(
                    "校验中：{}，接收 {} / {}，落盘 {}",
                    status.label, status.received, status.total, status.payload_flushed
                );
                let _ = tx.send(WorkerMsg::Progress(0.97, label.clone()));
                send_log(tx, LogLevel::Info, label);
            }
            Err(err) => {
                send_log(
                    tx,
                    LogLevel::Warn,
                    format!("读取状态失败，设备可能正在重启：{err:#}"),
                );
            }
        }
        thread::sleep(Duration::from_millis(500));
    }

    bail!("等待 OTA 完成超时，请重新连接设备读取版本确认状态")
}

fn wait_for_receive_started(device: &HidDevice, package_size: usize) -> Result<OtaStatus> {
    let expected_size = u32::try_from(package_size).context("OTA 包超过 4GB，设备不支持")?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last = OtaStatus::default();
    while Instant::now() < deadline {
        let status = read_ota_status(device)?;
        ensure_status_ok(&status)?;
        if status.status == 1 && status.total == expected_size {
            return Ok(status);
        }
        last = status;
        thread::sleep(Duration::from_millis(100));
    }

    bail!(
        "设备没有进入 OTA 接收状态：{}，接收 {} / {}，落盘 {}，raw {}。START 命令可能没有被固件接收",
        last.label,
        last.received,
        last.total,
        last.payload_flushed,
        last.raw_hex
    )
}

fn ensure_transfer_status(status: &OtaStatus, package_size: usize) -> Result<()> {
    let expected_size = u32::try_from(package_size).context("OTA 包超过 4GB，设备不支持")?;
    if status.status == 0 && status.total == 0 {
        bail!(
            "设备 OTA 状态仍为空：START/DATA 命令可能没有被固件接收，raw {}",
            status.raw_hex
        );
    }
    if status.total != 0 && status.total != expected_size {
        bail!(
            "设备 OTA 总大小异常：设备 {} bytes，软件 {} bytes",
            status.total,
            expected_size
        );
    }
    Ok(())
}

fn ensure_status_ok(status: &OtaStatus) -> Result<()> {
    if status.status >= 224 || status.status == 255 {
        bail!(
            "设备 OTA 失败：{}，细节：{}({})，地址 0x{:08x}，raw {}",
            status.label,
            status.error_label,
            status.error_detail,
            status.error_address,
            status.raw_hex
        );
    }
    Ok(())
}

fn resolve_latest_release(repo: &str, speed: Speed) -> Result<ReleaseInfo> {
    let repo = normalize_repo(repo)?;
    let client = github_client()?;
    let latest_url = format!("https://github.com/{repo}/releases/latest");
    let response = client
        .get(&latest_url)
        .send()
        .with_context(|| format!("请求 latest 地址失败：{latest_url}"))?
        .error_for_status()
        .context("GitHub latest 地址返回失败状态")?;
    let tag = extract_tag_from_url(response.url())?;
    Ok(release_info_from_tag(&repo, &tag, speed))
}

fn release_info_from_tag(repo: &str, tag: &str, speed: Speed) -> ReleaseInfo {
    let safe_tag = tag.replace('/', "-");
    let suffix = if speed == Speed::Hs { "-hs" } else { "" };
    let ota_name = format!("ds5dongle-lctech616-{safe_tag}{suffix}.bin.ota");
    let checksum_name = format!("SHA256SUMS-{safe_tag}-{}.txt", speed.as_str());
    let base = format!("https://github.com/{repo}/releases/download/{tag}");
    ReleaseInfo {
        tag: tag.to_string(),
        ota_url: format!("{base}/{ota_name}"),
        checksum_url: format!("{base}/{checksum_name}"),
        ota_name,
        checksum_name,
    }
}

fn infer_speed_from_version(version: &str) -> Speed {
    let upper = version.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    for index in 1..bytes.len() {
        if bytes[index] == b'H' && bytes[index - 1].is_ascii_digit() {
            return Speed::Hs;
        }
    }
    Speed::Fs
}

fn download_release_package(
    latest: &ReleaseInfo,
    tx: &Sender<WorkerMsg>,
) -> Result<DownloadedPackage> {
    let client = github_client()?;
    send_log(
        tx,
        LogLevel::Info,
        format!("下载校验文件：{}", latest.checksum_name),
    );
    let checksum_text = match client.get(&latest.checksum_url).send() {
        Ok(response) if response.status().is_success() => {
            Some(response.text().context("读取校验文件失败")?)
        }
        Ok(response) => {
            send_log(
                tx,
                LogLevel::Warn,
                format!("校验文件不可用：HTTP {}", response.status()),
            );
            None
        }
        Err(err) => {
            send_log(tx, LogLevel::Warn, format!("校验文件下载失败：{err:#}"));
            None
        }
    };

    send_log(
        tx,
        LogLevel::Info,
        format!("下载 OTA 包：{}", latest.ota_name),
    );
    let bytes = client
        .get(&latest.ota_url)
        .send()
        .with_context(|| format!("下载 OTA 包失败：{}", latest.ota_url))?
        .error_for_status()
        .context("OTA 包下载地址返回失败状态")?
        .bytes()
        .context("读取 OTA 包内容失败")?
        .to_vec();

    let sha256 = sha256_hex(&bytes);
    if let Some(text) = checksum_text {
        if let Some(expected) = checksum_for_name(&text, &latest.ota_name) {
            if expected != sha256 {
                bail!("OTA 包 SHA256 不匹配：期望 {expected}，实际 {sha256}");
            }
            send_log(tx, LogLevel::Success, "SHA256 校验通过");
        } else {
            send_log(
                tx,
                LogLevel::Warn,
                "校验文件里没有找到这个 OTA 包名，已跳过校验比对",
            );
        }
    }

    Ok(DownloadedPackage {
        name: latest.ota_name.clone(),
        bytes,
        sha256,
    })
}

fn github_client() -> Result<Client> {
    Client::builder()
        .user_agent("ds5dongle-ota-updater/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("创建 HTTP 客户端失败")
}

fn normalize_repo(repo: &str) -> Result<String> {
    let repo = repo
        .trim()
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let mut parts = repo.split('/');
    let owner = parts.next().filter(|value| !value.is_empty());
    let name = parts.next().filter(|value| !value.is_empty());
    if owner.is_none() || name.is_none() || parts.next().is_some() {
        bail!("仓库格式不正确，请填写 owner/repo 或 GitHub 仓库地址");
    }
    Ok(format!("{}/{}", owner.unwrap(), name.unwrap()))
}

fn extract_tag_from_url(url: &Url) -> Result<String> {
    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| anyhow!("latest 跳转地址没有 path：{url}"))?
        .collect();
    for window in segments.windows(2) {
        if window[0] == "tag" && !window[1].is_empty() {
            return Ok(window[1].to_string());
        }
    }
    bail!("无法从 latest 跳转地址解析 tag：{url}")
}

fn checksum_for_name(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        if hash.len() == 64 && file == name {
            Some(hash.to_ascii_lowercase())
        } else {
            None
        }
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn send_log(tx: &Sender<WorkerMsg>, level: LogLevel, text: impl Into<String>) {
    let _ = tx.send(WorkerMsg::Log(level, text.into()));
}

fn current_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let seconds = now % 86_400;
    let hour = (seconds / 3_600 + 8) % 24;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{hour:02}:{minute:02}:{second:02}")
}

fn status_label(status: u8) -> &'static str {
    match status {
        0 => "idle",
        1 => "receiving",
        2 => "verifying",
        3 => "complete",
        224 => "generic error",
        225 => "checksum error",
        226 => "signature error",
        227 => "device error",
        228 => "image error",
        255 => "error",
        _ => "unknown",
    }
}

fn error_detail_label(detail: u8) -> &'static str {
    match detail {
        0 => "none",
        1 => "partition lookup failed",
        2 => "active partition table failed",
        3 => "FW entry failed",
        4 => "partition switch failed",
        5 => "bad OTA magic",
        6 => "bad OTA type",
        7 => "bad OTA size",
        8 => "image too large",
        9 => "flash erase failed",
        10 => "flash write failed",
        11 => "flash readback failed",
        12 => "flash compare mismatch",
        13 => "payload too large",
        14 => "bad payload magic",
        15 => "SHA init failed",
        16 => "SHA update failed",
        17 => "SHA finish failed",
        18 => "SHA mismatch",
        19 => "package too small",
        20 => "incomplete package",
        21 => "incomplete flash",
        22 => "sequence mismatch",
        23 => "short command",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_repo_forms() {
        assert_eq!(
            normalize_repo("mainlxl/ds5dongle-bl618-opensource").unwrap(),
            "mainlxl/ds5dongle-bl618-opensource"
        );
        assert_eq!(
            normalize_repo("https://github.com/mainlxl/ds5dongle-bl618-opensource.git").unwrap(),
            "mainlxl/ds5dongle-bl618-opensource"
        );
    }

    #[test]
    fn builds_release_asset_names() {
        let info = release_info_from_tag("mainlxl/ds5dongle-bl618-opensource", "v3.18", Speed::Hs);
        assert_eq!(info.ota_name, "ds5dongle-lctech616-v3.18-hs.bin.ota");
        assert_eq!(info.checksum_name, "SHA256SUMS-v3.18-hs.txt");
    }

    #[test]
    fn infers_speed_from_firmware_version() {
        assert_eq!(infer_speed_from_version("LCT616-DS5 3.18"), Speed::Fs);
        assert_eq!(infer_speed_from_version("LCT616-DS5 3.18H"), Speed::Hs);
        assert_eq!(
            infer_speed_from_version("LCT616-DS5 3.18H-ota0.1f"),
            Speed::Hs
        );
    }

    #[test]
    fn parses_tag_from_latest_redirect_url() {
        let url =
            Url::parse("https://github.com/mainlxl/ds5dongle-bl618-opensource/releases/tag/v3.18")
                .unwrap();
        assert_eq!(extract_tag_from_url(&url).unwrap(), "v3.18");
    }

    #[test]
    fn parses_checksum_line() {
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  *ds5dongle-lctech616-v3.18-hs.bin.ota\n";
        assert_eq!(
            checksum_for_name(text, "ds5dongle-lctech616-v3.18-hs.bin.ota").unwrap(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn builds_short_feature_reports() {
        let start = make_start_command(1234).unwrap();
        let report = make_feature_report(&start, FeatureReportMode::Short).unwrap();
        assert_eq!(
            report,
            vec![REPORT_ID_CONFIG, OTA_CMD_START, 0xd2, 0x04, 0x00, 0x00]
        );
    }

    #[test]
    fn builds_padded_feature_reports() {
        let start = make_start_command(1234).unwrap();
        let report = make_feature_report(&start, FeatureReportMode::Padded64).unwrap();
        assert_eq!(report.len(), 64);
        assert_eq!(
            &report[..6],
            &[REPORT_ID_CONFIG, OTA_CMD_START, 0xd2, 0x04, 0x00, 0x00]
        );
        assert!(report[6..].iter().all(|byte| *byte == 0));
    }
}
