// 文档标题本地化（T4-09）的纯逻辑回归：无 React、无真实 document。
// 跑法：node --experimental-strip-types --test tests/*.test.ts
//
// 这一层要防的是**读侧白名单**被绕过：localStorage 里的 `language` 可能是
// 手改过的、旧版本残留的、或者是别的应用写进去的。
// 陌生值一旦被透传给 `document.title`，界面上就会出现一个来路不明的标题。
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_LANG,
  DOC_TITLES,
  LANGS,
  applyDocumentLanguage,
  docTitle,
  normalizeLang,
} from "../src/lib/i18n.ts";

test("读侧白名单：认得出的语言原样保留", () => {
  assert.equal(normalizeLang("zh-CN"), "zh-CN");
  assert.equal(normalizeLang("en-US"), "en-US");
  assert.equal(normalizeLang("  en-US  "), "en-US");
});

test("读侧白名单：陌生值一律回落默认，绝不透传", () => {
  assert.equal(normalizeLang("fr-FR"), DEFAULT_LANG);
  assert.equal(normalizeLang(""), DEFAULT_LANG);
  assert.equal(normalizeLang(null), DEFAULT_LANG);
  assert.equal(normalizeLang(undefined), DEFAULT_LANG);
  assert.equal(normalizeLang(42), DEFAULT_LANG);
  assert.equal(normalizeLang({}), DEFAULT_LANG);
  // 大小写不同也要能认（localStorage 里的值可能是人手打的）
  assert.equal(normalizeLang("EN-US"), DEFAULT_LANG);
});

test("标题按语言取值", () => {
  assert.equal(docTitle("zh-CN"), DOC_TITLES["zh-CN"]);
  assert.equal(docTitle("en-US"), DOC_TITLES["en-US"]);
  // 陌生语言拿到的是默认语言的标题，不是 undefined
  assert.equal(docTitle("fr-FR"), DOC_TITLES[DEFAULT_LANG]);
  assert.ok(DOC_TITLES[DEFAULT_LANG].length > 0);
});

test("applyDocumentLanguage：标题与 html lang 一次改完，不留半截状态", () => {
  // 造一个最小的 document 替身
  const fake = {
    title: "",
    documentElement: { lang: "" },
  } as unknown as Document;

  applyDocumentLanguage("en-US", fake);
  assert.equal(fake.title, DOC_TITLES["en-US"]);
  assert.equal(fake.documentElement.lang, "en-US");

  applyDocumentLanguage("fr-FR", fake);
  assert.equal(fake.title, DOC_TITLES[DEFAULT_LANG], "陌生语言要落到默认标题");
  assert.equal(fake.documentElement.lang, DEFAULT_LANG);
});

test("语言表里每一项都有标题（加语言时漏了标题会在这里被抓到）", () => {
  for (const lang of LANGS) {
    assert.ok(DOC_TITLES[lang.value], `语言 ${lang.value} 没有对应标题`);
    assert.ok(DOC_TITLES[lang.value].trim().length > 0);
    assert.ok(lang.label.trim().length > 0);
  }
});
