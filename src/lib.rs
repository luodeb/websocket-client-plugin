use futures_util::{SinkExt, StreamExt};
use plugin_interfaces::{
    create_plugin_interface_from_handler, log_info, log_warn,
    pluginui::{Context, Ui},
    PluginHandler, PluginInstanceContext, PluginInterface,
};
use std::sync::Arc;
use tokio::{runtime::Runtime, sync::Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// WebSocket 客户端连接状态
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}
type WebSocketSender = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
/// WebSocket 客户端插件实现
#[derive(Clone)]
pub struct WebSocketClientPlugin {
    connection_state: Arc<Mutex<ConnectionState>>,
    server_address: String,
    server_port: String,
    auto_reconnect: bool,
    runtime: Option<Arc<Runtime>>,
    client_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    ws_sender: Arc<Mutex<Option<WebSocketSender>>>,
}

impl WebSocketClientPlugin {
    fn new() -> Self {
        Self {
            connection_state: Arc::new(Mutex::new(ConnectionState::Disconnected)),
            server_address: "127.0.0.1".to_string(),
            server_port: "8080".to_string(),
            auto_reconnect: false,
            runtime: None,
            client_handle: Arc::new(Mutex::new(None)),
            ws_sender: Arc::new(Mutex::new(None)),
        }
    }

    /// 连接到 WebSocket 服务器（同步等待结果）
    async fn connect_to_server_sync(
        &self,
        plugin_ctx: PluginInstanceContext,
    ) -> Result<(), String> {
        *self.connection_state.lock().await = ConnectionState::Connecting;
        plugin_ctx.refresh_ui();

        let server_url = format!("ws://{}:{}", self.server_address, self.server_port);
        plugin_ctx.send_message_to_frontend(&format!("正在连接到服务器: {}", server_url));

        match connect_async(&server_url).await {
            Ok((ws_stream, _)) => {
                log_info!("WebSocket connection established to {}", server_url);
                *self.connection_state.lock().await = ConnectionState::Connected;
                plugin_ctx.send_message_to_frontend(&format!("已连接到服务器: {}", server_url));
                plugin_ctx.refresh_ui();

                let (ws_sender, mut ws_receiver) = ws_stream.split();
                *self.ws_sender.lock().await = Some(ws_sender);

                let connection_state = self.connection_state.clone();
                let ws_sender_ref = self.ws_sender.clone();
                let plugin_ctx_clone = plugin_ctx.clone();
                let auto_reconnect = self.auto_reconnect;

                // 启动消息接收任务
                let handle = tokio::spawn(async move {
                    while let Some(msg) = ws_receiver.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                log_info!("Received message from server: {}", text);
                                plugin_ctx_clone.send_message_to_frontend(text.as_str());
                            }
                            Ok(Message::Close(_)) => {
                                log_info!("Server closed connection");
                                *connection_state.lock().await = ConnectionState::Disconnected;
                                *ws_sender_ref.lock().await = None;
                                plugin_ctx_clone.send_message_to_frontend("服务器关闭了连接");
                                plugin_ctx_clone.call_disconnect();

                                // 自动重连逻辑
                                if auto_reconnect {
                                    log_info!("Attempting to reconnect...");
                                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                    // 这里可以添加重连逻辑
                                }
                                break;
                            }
                            Err(e) => {
                                log_warn!("WebSocket error: {}", e);
                                let error_msg = format!("连接错误: {}", e);
                                *connection_state.lock().await =
                                    ConnectionState::Error(error_msg.clone());
                                *ws_sender_ref.lock().await = None;
                                plugin_ctx_clone.send_message_to_frontend(&error_msg);
                                plugin_ctx_clone.refresh_ui();
                                break;
                            }
                            _ => {}
                        }
                    }

                    // 检测连接是否被远端关闭（接收流结束但状态仍为已连接）
                    if matches!(*connection_state.lock().await, ConnectionState::Connected) {
                        log_info!("Server disconnected (receiver stream ended)");
                        *connection_state.lock().await = ConnectionState::Disconnected;
                        *ws_sender_ref.lock().await = None;
                        plugin_ctx_clone.send_message_to_frontend("服务器断开了连接");
                        plugin_ctx_clone.call_disconnect();
                    }
                });

                // 保存任务句柄
                if let Ok(mut client_handle) = self.client_handle.try_lock() {
                    *client_handle = Some(handle);
                }

                Ok(())
            }
            Err(e) => {
                let error_msg = format!("连接失败: {}", e);
                log_warn!("Failed to connect to {}: {}", server_url, e);
                *self.connection_state.lock().await = ConnectionState::Error(error_msg.clone());
                plugin_ctx.send_message_to_frontend(&error_msg);
                plugin_ctx.refresh_ui();
                Err(error_msg)
            }
        }
    }

    /// 连接到 WebSocket 服务器
    /// 断开连接
    async fn disconnect(&self, plugin_ctx: &PluginInstanceContext) {
        if let Some(mut sender) = self.ws_sender.lock().await.take() {
            let _ = sender.close().await;
        }

        // 取消客户端任务
        if let Some(handle) = self.client_handle.lock().await.take() {
            handle.abort();
        }

        *self.connection_state.lock().await = ConnectionState::Disconnected;
        plugin_ctx.send_message_to_frontend("已断开连接");
        plugin_ctx.refresh_ui();
        log_info!("WebSocket client disconnected");
    }

    /// 发送消息到服务器
    async fn send_message_to_server(&self, message: &str) -> Result<(), String> {
        if let Some(mut sender) = self.ws_sender.lock().await.take() {
            let result = sender.send(Message::Text(message.to_string())).await;
            *self.ws_sender.lock().await = Some(sender);

            match result {
                Ok(_) => {
                    log_info!("Message sent to server: {}", message);
                    Ok(())
                }
                Err(e) => {
                    log_warn!("Failed to send message to server: {}", e);
                    Err(format!("发送失败: {}", e))
                }
            }
        } else {
            Err("未连接到服务器".to_string())
        }
    }

    /// 获取连接状态的显示文本
    async fn get_connection_status(&self) -> String {
        match &*self.connection_state.lock().await {
            ConnectionState::Disconnected => "未连接".to_string(),
            ConnectionState::Connecting => "连接中...".to_string(),
            ConnectionState::Connected => "已连接".to_string(),
            ConnectionState::Error(msg) => format!("错误: {}", msg),
        }
    }
}

impl PluginHandler for WebSocketClientPlugin {
    fn update_ui(&mut self, _ctx: &Context, ui: &mut Ui, _plugin_ctx: &PluginInstanceContext) {
        ui.label("WebSocket 客户端插件");

        // 服务器连接配置区域
        ui.horizontal(|ui| {
            ui.label("服务器地址:");
            let address_response = ui.text_edit_singleline(&mut self.server_address);
            if address_response.changed() {
                log_info!("Server address changed to: {}", self.server_address);
            }
        });

        ui.horizontal(|ui| {
            ui.label("服务器端口:");
            let port_response = ui.text_edit_singleline(&mut self.server_port);
            if port_response.changed() {
                log_info!("Server port changed to: {}", self.server_port);
            }
        });

        // 连接状态显示
        ui.horizontal(|ui| {
            ui.label("连接状态:");
            if let Some(runtime) = &self.runtime {
                let status = runtime.block_on(self.get_connection_status());
                ui.label(&status);
            } else {
                ui.label("运行时未初始化");
            }
        });

        ui.label(""); // 空行

        // 自动重连选项
        ui.horizontal(|ui| {
            ui.label("自动重连:");
            // 简单的文本显示，因为没有 checkbox 方法
            ui.label(if self.auto_reconnect {
                "已启用"
            } else {
                "已禁用"
            });
        });
    }

    fn on_mount(
        &mut self,
        plugin_ctx: &PluginInstanceContext,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let metadata = plugin_ctx.get_metadata();
        log_info!(
            "[{}] WebSocket Client Plugin mounted successfully",
            metadata.name
        );

        // 初始化 tokio 异步运行时
        match Runtime::new() {
            Ok(runtime) => {
                self.runtime = Some(Arc::new(runtime));
                log_info!("Tokio runtime initialized successfully");
            }
            Err(e) => {
                log_warn!("Failed to initialize tokio runtime: {}", e);
            }
        }

        Ok(())
    }

    fn on_dispose(
        &mut self,
        plugin_ctx: &PluginInstanceContext,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let metadata = plugin_ctx.get_metadata();
        log_info!("[{}] WebSocket Client Plugin disposing", metadata.name);

        // 断开连接
        if let Some(runtime) = &self.runtime {
            let self_clone = self.clone();
            let plugin_ctx_clone = plugin_ctx.clone();
            runtime.spawn(async move {
                self_clone.disconnect(&plugin_ctx_clone).await;
            });
        }

        // 关闭异步运行时
        if let Some(runtime) = self.runtime.clone() {
            match Arc::try_unwrap(runtime) {
                Ok(runtime) => {
                    runtime.shutdown_timeout(std::time::Duration::from_millis(1000));
                    log_info!("Tokio runtime shutdown successfully");
                }
                Err(_) => {
                    log_warn!("Cannot shutdown runtime: other references still exist");
                }
            }
        }

        plugin_ctx.send_message_to_frontend("WebSocket 客户端插件已卸载");
        Ok(())
    }

    fn on_connect(
        &mut self,
        plugin_ctx: &PluginInstanceContext,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let metadata = plugin_ctx.get_metadata();
        log_info!("[{}] WebSocket Client Plugin connected", metadata.name);

        // 同步等待连接结果
        if let Some(runtime) = &self.runtime {
            let self_clone = self.clone();
            let plugin_ctx_clone = plugin_ctx.clone();

            let result = runtime
                .block_on(async move { self_clone.connect_to_server_sync(plugin_ctx_clone).await });

            match result {
                Ok(_) => Ok(()),
                Err(e) => Err(e.into()),
            }
        } else {
            Err("运行时未初始化".into())
        }
    }

    fn on_disconnect(
        &mut self,
        plugin_ctx: &PluginInstanceContext,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let metadata = plugin_ctx.get_metadata();
        log_info!("[{}] WebSocket Client Plugin disconnected", metadata.name);

        // 断开 WebSocket 连接
        if let Some(runtime) = &self.runtime {
            let self_clone = self.clone();
            let plugin_ctx_clone = plugin_ctx.clone();
            runtime.spawn(async move {
                self_clone.disconnect(&plugin_ctx_clone).await;
            });
        }
        Ok(())
    }

    fn handle_message(
        &mut self,
        message: &str,
        plugin_ctx: &PluginInstanceContext,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let metadata = plugin_ctx.get_metadata();
        log_info!("[{}] Received message: {}", metadata.name, message);

        if let Some(runtime) = &self.runtime {
            let self_clone = self.clone();
            let message_owned = message.to_string();
            let plugin_ctx_clone = plugin_ctx.clone();

            runtime.spawn(async move {
                match self_clone.send_message_to_server(&message_owned).await {
                    Err(e) => {
                        plugin_ctx_clone.send_message_to_frontend(&format!("发送消息失败: {}", e));
                    }
                    Ok(_) => {
                        log_info!("Message sent to server successfully");
                    }
                }
            });
        }
        Ok("".to_string())
    }
}

/// 创建插件实例的导出函数
#[no_mangle]
pub extern "C" fn create_plugin() -> *mut PluginInterface {
    let plugin = WebSocketClientPlugin::new();
    let handler: Box<dyn PluginHandler> = Box::new(plugin);
    create_plugin_interface_from_handler(handler)
}

/// 销毁插件实例的导出函数
///
/// # Safety
///
/// This function is unsafe because it dereferences raw pointers.
/// The caller must ensure that:
/// - `interface` is a valid pointer to a `PluginInterface` that was created by `create_plugin`
/// - `interface` has not been freed or destroyed previously
/// - The `PluginInterface` and its associated plugin instance are in a valid state
#[no_mangle]
pub unsafe extern "C" fn destroy_plugin(interface: *mut PluginInterface) {
    if !interface.is_null() {
        ((*interface).destroy)((*interface).plugin_ptr);
        let _ = Box::from_raw(interface);
    }
}
