# Gemini Rust Gateway Cookie Sync

Cookie 由扩展通过 `chrome.cookies` 从浏览器自动读取，**不需要手动复制粘贴，也不需要打开开发者工具**。扩展中填写的内容只用于指明网关地址和这份 Cookie 归属哪个账号。

1. 在 `.env` 设置 `GEMINI_COOKIE_SYNC_ENABLED=true` 并启动网关。网关未运行时同步会失败并提示无法连接。
2. Chrome 打开 `chrome://extensions`，启用“开发者模式”。
3. 选择“加载已解压的扩展程序”，指向本目录。
4. 登录 `https://gemini.google.com/u/0/app`（或其他账号序号）。
5. 打开扩展，填写管理令牌、稳定账号名称、账号序号和轮换权重，点击同步。管理令牌即 `init` 写入项目 `.env` 的 `GEMINI_ADMIN_TOKEN`，与 Google 账号无关。
6. 可选启用每 30 分钟自动同步；该模式需要在扩展本地存储管理令牌。

日常续期不依赖本扩展：`__Secure-1PSIDTS` 由 Google 轮换后通过 `Set-Cookie` 下发，网关会在请求成功时接住并写回账号文件。扩展的作用是首次注入凭据，以及会话彻底失效后重新授权。自动同步是一层额外保险，在网关长期空闲、没有请求可承载轮换时仍能把浏览器里的最新 Cookie 推过去。

扩展只允许向 `http://127.0.0.1` 或 `http://localhost` 发送 Cookie。网关会先在线验证 Cookie，再创建独立账号会话并原子写入被 Git 忽略的账号文件。重复使用同一账号名称会更新账号；新名称会新增账号并参与自动轮换。不开启自动同步时，管理令牌默认只保存在 Chrome 扩展会话存储中。
