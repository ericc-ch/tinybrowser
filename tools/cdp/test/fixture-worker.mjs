import { writeFileSync } from "node:fs";

const input = JSON.parse(process.env.TINYBROWSER_CDP_WORKER_INPUT);
const pidFile = process.env.TINYBROWSER_CDP_FAKE_PID;
if (pidFile) writeFileSync(pidFile, String(process.pid));

if (input.id === "plain/runtime/runtime-evaluate-side-effect-free-onerror.js") {
  process.send({ result: { id: input.id, status: "PASS" }, type: "result" });
} else if (input.id === "plain/sessions/runtime-evaluate.js") {
  process.exit(7);
} else {
  setInterval(() => {}, 10_000);
}
