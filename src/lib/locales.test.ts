import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

/**
 * 两份界面文案的一致性守卫。
 *
 * AGENTS.md 要求「两边必须 1:1，改完跑一遍扁平化比对，差集为空才算过」，但那条口令过去
 * 只能靠人执行，漏一个 key 的后果是界面上直接显示原始 key（`deploy.fooBar`）而不是报错。
 * 这里把口令变成会红的用例：新增 / 删除文案时，两侧不同步就过不了 `npm test`。
 */

type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

const FILES = ["zh-CN", "en-US"] as const;
type LocaleName = (typeof FILES)[number];

function readLocale(name: LocaleName): string {
  return readFileSync(fileURLToPath(new URL(`../locales/${name}.json`, import.meta.url)), "utf8");
}

/** 扁平化成 `a.b.c` 点号路径，与 AGENTS.md 里手工比对的口径一致。 */
function flatten(
  value: JsonValue,
  prefix = "",
  out = new Map<string, JsonValue>(),
): Map<string, JsonValue> {
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    for (const [key, child] of Object.entries(value)) {
      flatten(child, prefix === "" ? key : `${prefix}.${key}`, out);
    }
    return out;
  }
  out.set(prefix, value);
  return out;
}

const texts = Object.fromEntries(
  FILES.map((name) => [name, readLocale(name)] as const),
) as Record<LocaleName, string>;
const flat = Object.fromEntries(
  FILES.map((name) => [name, flatten(JSON.parse(texts[name]) as JsonValue)] as const),
) as Record<LocaleName, Map<string, JsonValue>>;

describe("界面文案", () => {
  it("两份文案的 key 一一对应", () => {
    const zhKeys = new Set(flat["zh-CN"].keys());
    const enKeys = new Set(flat["en-US"].keys());

    // 分开断言，失败信息里能直接看出是哪一侧少了（差集为空才算过）。
    expect([...zhKeys].filter((key) => !enKeys.has(key))).toEqual([]);
    expect([...enKeys].filter((key) => !zhKeys.has(key))).toEqual([]);

    // 兜住「文件被清空 / 截断」这种整体性损坏：比逐条清单更早发现问题。
    expect(zhKeys.size).toBeGreaterThan(900);
  });

  it("没有空值", () => {
    for (const name of FILES) {
      const empty = [...flat[name]]
        .filter(([, value]) => value === "")
        .map(([key]) => key);
      expect(empty, `${name} 存在空字符串值`).toEqual([]);
    }
  });

  it("en-US 里不残留中文（除语言自称）", () => {
    // 语言名按母语书写是惯例，是唯一允许出现汉字的地方。
    const selfNames = new Set(["简体中文"]);
    const leaked = [...flat["en-US"]]
      .filter(
        ([, value]) =>
          typeof value === "string" &&
          /[\u4e00-\u9fff]/.test(value) &&
          !selfNames.has(value),
      )
      .map(([key, value]) => `${key} = ${String(value)}`);
    expect(leaked, "en-US 里有未翻译的中文").toEqual([]);
  });

  it("保持 2 空格缩进与统一行尾", () => {
    for (const name of FILES) {
      const text = texts[name];
      expect(text.startsWith("\uFEFF"), `${name} 不应有 BOM`).toBe(false);
      // 制表符会让 AGENTS.md 里那条「整份 re-dump 逐字节一致」的校验失效。
      expect(text.includes("\t"), `${name} 不应出现制表符`).toBe(false);

      // 行尾必须统一：CRLF 与 LF 混排正是「文本模式写回」留下的痕迹，也是往返检查
      // 看着通过、写回却改掉整份文件的那种事故。这里不硬钉 CRLF —— Linux 检出会把
      // 行尾折成 LF，钉死会让别人的机器假失败；要钉死得先加 .gitattributes。
      const crlf = (text.match(/\r\n/g) ?? []).length;
      const bareLf = (text.match(/(?<!\r)\n/g) ?? []).length;
      expect(
        crlf === 0 || bareLf === 0,
        `${name} 混用了 CRLF 与 LF（CRLF ${crlf} 处 / 裸 LF ${bareLf} 处）`,
      ).toBe(true);

      // 末尾必须恰好一个换行：用正则而不是 endsWith("\n\n")，后者在 CRLF 文件上
      // 永远为假（末尾是 \r\n），那条断言会等于没写。
      expect(/\r?\n$/.test(text), `${name} 末尾缺少换行`).toBe(true);
      expect(/(\r?\n){2}$/.test(text), `${name} 末尾多了一个换行`).toBe(false);

      const badIndent = text
        .split(/\r?\n/)
        .filter((line) => ((/^ */.exec(line)?.[0].length ?? 0) % 2) !== 0);
      expect(badIndent, `${name} 存在非 2 空格倍数的缩进`).toEqual([]);
    }
  });
});
