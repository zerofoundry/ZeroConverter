import test from "node:test";
import assert from "node:assert/strict";
import {
  isAllowed, isVisible, migrateLegacySettings, optionSpecsFor, requestOptions, valueOf, withValue,
} from "./outputOptions.mjs";

// Shaped like get_app_capabilities; the option specs are the engine's.
const quality = { key: "quality", type: "integer", control: "slider", min: 1, max: 100, default: 90, scope: "format", quick: true };
const resizeMode = { key: "resize_mode", type: "string", control: "choice", values: ["none", "max_width", "fit_box"], default: "none", scope: "family", quick: false };
const resizeWidth = { key: "resize_width", type: "integer", control: "number", min: 1, max: 16384, presets: [], default: null, unit: "px", scope: "family", quick: false, show_when: { key: "resize_mode", values: ["max_width", "fit_box"] } };
const bitrate = { key: "bitrate_kbps", type: "integer", control: "number", min: 32, max: 512, presets: [128, 320], default: 320, unit: "kbps", scope: "format", quick: true };
const channels = { key: "channels", type: "integer", control: "choice", values: [null, 1], default: null, scope: "family", quick: false };

const capabilities = {
  families: [
    {
      family: "image",
      descriptors: [
        { id: "png", canonical_extension: "png", aliases: [], can_read: true, can_write: true, options: [resizeMode, resizeWidth] },
        { id: "jpg", canonical_extension: "jpg", aliases: [], can_read: true, can_write: true, options: [quality, resizeMode, resizeWidth] },
        { id: "svg", canonical_extension: "svg", aliases: [], can_read: true, can_write: false, options: [] },
      ],
      routes: [],
    },
    {
      family: "audio",
      descriptors: [
        { id: "mp3", canonical_extension: "mp3", aliases: [], can_read: true, can_write: true, options: [bitrate, channels] },
      ],
      routes: [],
    },
  ],
};

test("specs come from the target's descriptor, none for read-only or unknown targets", () => {
  assert.deepEqual(optionSpecsFor(capabilities, "image", "jpg").map((s) => s.key), ["quality", "resize_mode", "resize_width"]);
  assert.deepEqual(optionSpecsFor(capabilities, "image", "svg"), []);
  assert.deepEqual(optionSpecsFor(capabilities, "image", "nope"), []);
  assert.deepEqual(optionSpecsFor(null, "image", "jpg"), []);
});

test("unset options use the engine's default", () => {
  assert.equal(valueOf(quality, {}, "image", "jpg"), 90);
  assert.equal(valueOf(channels, {}, "audio", "mp3"), null);
});

test("format-scoped values are per target, family-scoped ones are shared", () => {
  let values = withValue({}, quality, "image", "jpg", 60);
  values = withValue(values, resizeMode, "image", "jpg", "max_width");
  assert.equal(valueOf(quality, values, "image", "jpg"), 60);
  assert.equal(valueOf(quality, values, "image", "avif"), 90, "AVIF keeps its own quality");
  assert.equal(valueOf(resizeMode, values, "image", "png"), "max_width", "resize is shared by every image target");
});

test("a stored value the engine no longer allows falls back to the default", () => {
  const values = { "image:jpg": { quality: 400 }, image: { resize_mode: "stretch" } };
  assert.equal(valueOf(quality, values, "image", "jpg"), 90);
  assert.equal(valueOf(resizeMode, values, "image", "jpg"), "none");
  assert.equal(isAllowed(bitrate, 160), true, "any bitrate in range, not only the presets");
  assert.equal(isAllowed(bitrate, null), false, "MP3 has a fixed default, not 'automatic'");
  assert.equal(isAllowed(channels, 2), false);
});

test("a request carries the options in play and nothing else", () => {
  const specs = optionSpecsFor(capabilities, "image", "jpg");
  assert.deepEqual(requestOptions(specs, {}, "image", "jpg"), { quality: 90, resize_mode: "none" });
  const values = withValue(withValue({}, resizeMode, "image", "jpg", "max_width"), resizeWidth, "image", "jpg", 800);
  assert.equal(isVisible(resizeWidth, specs, values, "image", "jpg"), true);
  assert.deepEqual(requestOptions(specs, values, "image", "jpg"), { quality: 90, resize_mode: "max_width", resize_width: 800 });
  // PNG has no quality: none is sent, whatever was chosen for JPG.
  assert.deepEqual(requestOptions(optionSpecsFor(capabilities, "image", "png"), values, "image", "png"), { resize_mode: "max_width", resize_width: 800 });
});

test("settings from 0.1.x become option values", () => {
  const migrated = migrateLegacySettings({
    quality: 70,
    resizeMode: "fit_box", resizeWidth: 256, resizeHeight: 256,
    formatOptions: { bitrates: { mp3: 192 }, sampleRate: 48000, mono: true, ddsFormat: "bc1", ddsMipmaps: true },
  });
  assert.equal(migrated["image:jpg"].quality, 70);
  assert.deepEqual(migrated.image, { resize_mode: "fit_box", resize_width: 256, resize_height: 256 });
  assert.equal(migrated["audio:mp3"].bitrate_kbps, 192);
  assert.deepEqual(migrated.audio, { sample_rate: 48000, channels: 1 });
  assert.deepEqual(migrated["image:dds"], { dds_format: "bc1", dds_mipmaps: true });
  // The old app default (80) was never a choice: the engine's 90 applies.
  assert.deepEqual(migrateLegacySettings({ quality: 80, resizeMode: "none" }), {});
  // Already migrated: kept as is.
  const current = { image: { resize_mode: "max_width" } };
  assert.equal(migrateLegacySettings({ optionValues: current }), current);
});
