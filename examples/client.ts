/** Minimal TypeScript client for every local gateway capability (Node 18+). */

const baseUrl = "http://127.0.0.1:8000/v1";
const apiKey = "local-gemini-key"; // Must match GEMINI_API_KEY in .env.

type GenerationType =
  | "image"
  | "video"
  | "audio"
  | "music"
  | "deep_research"
  | "canvas";

type Task = {
  id: string;
  status: "queued" | "running" | "completed" | "failed" | "interrupted" | "canceled";
  progress?: unknown;
  result?: {
    text?: string;
    files?: Array<{ name: string; url: string; content_type: string }>;
  };
  error?: string;
};

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  headers.set("Authorization", `Bearer ${apiKey}`);
  headers.set("Content-Type", "application/json");
  const response = await fetch(`${baseUrl}${path}`, {
    ...init,
    headers,
  });
  if (!response.ok) throw new Error(`${response.status}: ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function chat(prompt: string): Promise<string> {
  const data = await request<{
    choices: Array<{ message: { content: string } }>;
  }>("/chat/completions", {
    method: "POST",
    body: JSON.stringify({
      model: "gemini-3-flash",
      messages: [{ role: "user", content: prompt }],
    }),
  });
  return data.choices[0].message.content;
}

async function generate(type: GenerationType, prompt: string): Promise<Task> {
  const task = await request<Task>("/generations", {
    method: "POST",
    body: JSON.stringify({ type, prompt }),
  });

  for (;;) {
    const current = await request<Task>(`/tasks/${task.id}`);
    if (current.status === "completed") return current;
    if (["failed", "interrupted", "canceled"].includes(current.status)) {
      throw new Error(current.error ?? `Task ${current.status}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 3000));
  }
}

console.log(await chat("Introduce yourself in one sentence."));
const image = await generate("image", "A cinematic orange cat astronaut");
console.log(image.result?.files);
