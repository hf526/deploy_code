import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // 只测 src/lib 下的纯逻辑：组件/交互仍靠 tsc + 手工验证。
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
