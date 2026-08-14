# Gemini Rust Gateway Cookie Sync

1. 在 `.env` 设置 `GEMINI_COOKIE_SYNC_ENABLED=true` 并启动网关。
2. Chrome 打开 `chrome://extensions`，启用“开发者模式”。
3. 选择“加载已解压的扩展程序”，指向本目录。
4. 登录 `https://gemini.google.com/u/0/app`（或其他账号序号）。
5. 打开扩展，填写 `GEMINI_ADMIN_TOKEN`、稳定账号名称、账号序号和轮换权重，点击同步。
6. 可选启用每 30 分钟自动同步；该模式需要在扩展本地存储管理令牌。

扩展只允许向 `http://127.0.0.1` 或 `http://localhost` 发送 Cookie。网关会先在线验证 Cookie，再创建独立账号会话并原子写入被 Git 忽略的账号文件。重复使用同一账号名称会更新账号；新名称会新增账号并参与自动轮换。不开启自动同步时，管理令牌默认只保存在 Chrome 扩展会话存储中。
