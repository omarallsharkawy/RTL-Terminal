# 🔍 تدقيق كود + فحص سيكيوريتي — Twitty (RTL-Terminal)

**المشروع:** `omarallsharkawy/RTL-Terminal` — التطبيق **Twitty** ‏(Native Rust terminal emulator — wgpu + cosmic-text + alacritty_terminal، تخصصه عرض العربي والـ RTL/BiDi صح)
**الـ commit المراجَع:** `68e215e810da366bec057013a46d38fc003ff574` من `main` ‏(2026-10-16)
**نوع المراجعة:** Code Audit كامل (ميثاق Coding Auditor) + Security Scan (Cloudflare `security-audit` skill — بتركيز على companions: ‏`MEMORY-SAFETY-AND-BINARY` و `SUPPLY-CHAIN-AND-RELEASE`، لأن الهدف native/binary وبيعالج untrusted byte streams)
**التاريخ:** 2026-10-23

> الـ PR ده بيضيف ملف التقرير ده بس (`docs/audit/CODE-AUDIT-REPORT.md`). **مفيش أي تعديل على كود التطبيق نفسه.**

---

## 1. الحكم العام (Executive Summary)

⚠️ **الكود ده من أنضف ما راجعت — disciplined بشكل غير معتاد — بس فيه مشكلتين Major في الـ lifecycle ومشكلتين سيكيوريتي Medium لازم يتاخدوا بجدية.**

- **مفيش `unsafe` أصلًا في الكود:** `#![forbid(unsafe_code)]` في `src/lib.rs:1` و `src/main.rs:3` — فئة كاملة من ثغرات الـ memory-safety مقفولة بالتصميم.
- **الـ panics شبه معدومة:** ‏7 `unwrap()` في `src/` كلها — صفر في كود الإنتاج في المسارات الحساسة (5 في tests، و2 محروسة بـ filter).
- المشاكل الحقيقية مش في الـ parsing (ده متساب لـ `alacritty_terminal` الناضج ومتغطي بـ fuzz targets) — المشاكل في **دورة حياة الـ PTY/process** (zombie shells و channels غير محدودة) وفي **سلسلة إمداد الـ release** (أكشنز مش مثبتة بـ SHA + باينري مش موقّع + checksum من نفس المصدر).

### الأعداد

**Code Audit (حسب الـ severity):**

| Severity | العدد |
|---|---|
| 🔴 Critical | 0 |
| 🟠 Major | 2 |
| 🟡 Minor | 4 |
| 🔵 Suggestion | 2 |
| **الإجمالي** | **8** |

**Security Scan (حسب الـ verdict بتاع الـ skill):**

| Verdict | العدد |
|---|---|
| ✅ Confirmed | 2 (الاتنين Medium) |
| 🟡 Needs Validation | 5 |
| ❌ Rejected | 6 claims اتفندت |

---

## 2. التغطية (Coverage)

اتراجع بقراءة كاملة سطر بسطر:

- **`src/` كلها (89 ملف Rust، ‏~13,914 سطر)** مع تركيز عميق على: `pty.rs`، `session/` ‏(mod/pane/spawn/workspace)، `terminal.rs`، `config.rs`، `app/` ‏(mod/event/keyboard/mouse/panes/selection/window)، `input/`، `links.rs`، `ui/clipboard.rs`، `render/text/` ‏(bidi/segment/shaper)، `render/row/`، `layout.rs`، `main.rs`، `lib.rs`
- **البنية التحتية:** `Cargo.toml` + `Cargo.lock` ‏(378 package مقفولة بالـ lockfile)، `deny.toml`، `.github/workflows/build.yml` كلها، `.github/dependabot.yml`، `fuzz/` ‏(الهدفين)، `README.md` (قسم الـ install/release)
- **الـ tests:** جرد — 31 ملف test في `tests/` + unit tests جوه الـ modules، ‏~217 test function، منهم `property_tests` (proptest) و `core_audit_tests` و fuzz targets

**بره النطاق / حدود:** الـ dependencies نفسها من جوه (wgpu, cosmic-text, alacritty_terminal 0.26, portable-pty 0.9) — اتفحصت من ناحية الاستخدام والـ versions بس، مش source audit. سلوك الـ OS/windowing وقت التشغيل مش متاح للفحص الثابت.

**⚠️ حدود التشغيل:** **لم يتم تشغيل `cargo check` / `cargo test` / `cargo audit`** — مفيش Rust toolchain في بيئة التدقيق. كل الـ findings من قراءة وتتبع الـ source، واللي محتاج تشغيل عشان يتأكد متعلم `needs_validation` صراحة.

---

## 3. الـ Code Audit — الـ Findings بالتفصيل

### 🟠 A1 ‏[Major] اكتشاف موت الـ shell مربوط بـ EOF بتاع الـ PTY: zombie shell و pane مبيقفلش و thread/fd بيتسربوا

**المكان:** `src/pty.rs:213-227` ‏(الـ reader thread)، `src/pty.rs:189-197`، `src/pty.rs:122-126` ‏(`Drop for Pty`)

**المشكلة:** الـ reader thread هو المكان **الوحيد** اللي الـ child بيحصله `wait()` وبيتبلغ عن خروجه — وده بيحصل بس **بعد** ما حلقة القراية تخلص:

```rust
thread::spawn(move || {
    loop { match reader.read(&mut buf) { Ok(0) => break, Ok(n) => {…}, Err(_) => break } }
    let _ = child.wait();   // ← بيوصله بس عند EOF
    on_exit();
});
```

والـ reader شايل نسخة **cloned من الـ master fd**، والـ `Drop` بيعمل `output.close()` + `killer.kill()` بس — معندوش أي طريقة يقفل الـ reader أو يعمله join.

**ليه ده bug:** شغّل `sleep 600 &` (أو أي daemon/background job وارث الـ stdio) وبعدين `exit`، أو اقفل الـ pane ولسه فيه job في الخلفية ماسك الـ slave fd. الـ shell مات فعلًا، بس الـ slave لسه مفتوح → الـ `read()` عمره ما بيرجع EOF → `child.wait()` مبيتنفذش → الـ shell يفضل **zombie**، وإيفنت `PtyExit` مبيحصلش → الـ pane/tab مبيقفلش تلقائيًا أبدًا. ومع القفل اليدوي، الـ reader thread والـ fd بتوعه بيفضلوا عايشين للأبد وبيصرفوا output على فاضي. daemon مبيخلصش = التسريب ده دائم.

**الـ Fix المقترح:** فك الارتباط بين الخروج والـ EOF — thread مستقل للـ `child.wait()` يبعت `on_exit`، والـ reader للـ output بس، ومسار shutdown للـ reader عند الـ drop (قفل الـ master مثلًا):

```rust
// waiter thread منفصل:
thread::spawn(move || { let _ = child.wait(); on_exit(); });
// والـ reader يفضل للقراية بس، والـ Drop يقفل الـ master عشان الـ read يفك.
```

---

### 🟠 A2 ‏[Major] قناة إدخال الـ PTY غير محدودة (unbounded channel): ذاكرة بتكبر بلا سقف لما البرنامج يبطل يقرا stdin

**المكان:** `src/pty.rs:198` ‏— `channel::<Vec<u8>>()` ‏(‏`std::sync::mpsc::channel` غير محدود)، `src/pty.rs:257-261` ‏(`Pty::write`)، `src/pty.rs:199-211` ‏(الـ writer thread)

**المشكلة:** اتجاه الـ output معموله bound بعناية (`OutputQueue::LIMIT = 4 MiB` في `src/pty.rs:65` مع شرح وافي)، بس اتجاه الـ **input معندوش أي bound**. الـ writer thread بيعلق في `write_all` لأجل غير مسمى أول ما بفر الـ PTY يتملي (برنامج مش بيقرا: `sleep`، job واقف بـ Ctrl-Z، تطبيق fullscreen متجاهل الإدخال).

**التريجر:** طول ما الـ child مش بيستهلك، كمّل تغذية: pastes كبيرة/متكررة وكبسات autorepeat عن طريق `Pane::write` ‏(`src/session/pane.rs:76-80`)، أو ردود `PtyWrite` اللي البرنامج نفسه بيستفزها (`src/app/event.rs:73`). كل بايت بيتنسخ للـ heap ويتراكم ومبيقعش أبدًا → نمو ذاكرة لا نهائي لحد الـ OOM، وجزء منه مدفوع من untrusted PTY stream. وبالمرة: writer thread معلق جوه `write_all` بيعيش بعد `Pty::drop` (مسار الخروج بقفل القناة مبيتوصلش) → تسريب thread + writer fd.

**الـ Fix المقترح:** قناة محدودة مع فشل ظاهر بدل الصامت:

```rust
let (writer, write_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
// في write():
writer.try_send(data.to_vec())
    .map_err(|_| anyhow::anyhow!("PTY input queue full"))
```

(الـ `send` الحاجب على قناة محدودة بديل، بس ممكن يجمّد الـ UI thread؛ الـ `try_send` + ‏Err يتلوجّ أأمن.)

---

### 🟡 A3 ‏[Minor] فشل إعداد الـ PTY بعد spawn ناجح بيسيب shell يتيم — لا kill ولا wait

**المكان:** `src/pty.rs:183-197`

**المشكلة:** ‏`spawn_command` بينجح (~سطر 184)، وبعدها `try_clone_reader()` ‏(192) و `take_writer()` ‏(196) ممكن يرجعوا بـ `?` بدري. على المسارات دي الـ `child` المحلي بيخرج بره الـ scope وبس: الكود عمره ما بينده `kill()` ولا `wait()` عليه، والـ threads اللي فيهم الـ `wait()` الوحيد لسه متبدأتش.

**التريجر:** أي خطأ في الخطوتين دول بعد الـ spawn — أرجحها fd exhaustion — والـ shell المتساب بيخلي المشكلة الأصلية أسوأ.

**الـ Fix:** قبل إرجاع الخطأ، ريّب صراحة: رتّب الكود بحيث مسار الخطأ يعمل `let _ = child.kill(); let _ = child.wait();`.

---

### 🟡 A4 ‏[Minor] ملف config موجود بس مش بيتقري بيتعامل كأنه مش موجود — وممكن يتكتب فوقه الـ defaults

**المكان:** `src/config.rs:155-161` ‏(`load_from`)، مقابل فرع خطأ الـ parse في `src/config.rs:162-170` والـ `save_to` في `src/config.rs:200-212`؛ الكتابة بتحصل من `src/app/mod.rs:174-177`

**المشكلة:** الحماية الموثقة — «ملف فشل في الـ parse مبيتكتبش فوقه أبدًا» — مطبقة بس في فرع `serde_json::from_str` (بيظبط `keep_file: true`). أي خطأ في `fs::read_to_string` بيرجع `Self::default()` عادي بـ `keep_file: false`:

```rust
let Ok(content) = fs::read_to_string(path) else { return Self::default(); };
```

ده بيخلط `NotFound` (آمن) بـ invalid UTF-8 و permission errors وأخطاء I/O — الملف موجود وفيه محتوى المستخدم بس عمره ما وصل للـ parser.

**التريجر:** اكتب bytes مش UTF-8 (أو خليه unreadable) في `~/.config/twitty/config.json`، شغّل Twitty، غيّر الزوم/الشفافية مرة (بيظبط `config_dirty`)، اقفل — `config.save()` بيعمل rename atomic بالـ defaults فوق ملف المستخدم الأصلي. **ضياع بيانات صامت.**

**الـ Fix:** فرّق بنوع الخطأ:

```rust
let content = match fs::read_to_string(path) {
    Ok(c) => c,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
    Err(e) => {
        warn(format!("twitty: cannot read {} ({e}); it will not be overwritten", path.display()));
        return Self { keep_file: true, ..Self::default() };
    }
};
```

---

### 🟡 A5 ‏[Minor] ‏`Pane::resize` بيخلي winsize الـ PTY يفقد التزامن نهائيًا بعد فشل واحد، وأخطاء الـ write/resize بتتبلع من غير أي log

**المكان:** `src/session/pane.rs:76-92`

**المشكلة:**

```rust
pub fn write(&self, bytes: &[u8]) {
    if let Some(ref pty) = self.pty { let _ = pty.write(bytes); }   // ← الخطأ بيترمي
}
pub fn resize(&mut self, cols: usize, rows: usize) {
    if self.size == (cols, rows) { return; }                        // ← early return أبدي
    self.size = (cols, rows);                                       // ← الكاش بيتحفظ قبل العملية الفاشلة
    self.terminal.resize(cols, rows);
    if let Some(ref pty) = self.pty { let _ = pty.resize(cols as u16, rows as u16); }  // ← والخطأ بيترمي
}
```

الحجم الجديد بيتحفظ في الكاش **قبل** الـ PTY resize القابل للفشل، والخطأ بيتدفن. بعد كده `apply_layout` ينده `resize` بنفس الأبعاد فيضرب الـ early return للأبد → الـ shell شغال بـ winsize غلط والبرامج بترسم على جريد مش بتاعها، لحد ما حجم النافذة يتغير صدفة. ونفس النمط: لو الـ writer thread مات، كل كبسة بتختفي بصمت — `Pty::write` بيرجع `Err` وصفي وجاهز، ودي سلسلة النداء الوحيدة، ومفيش log في أي حتة.

**الـ Fix:** خزّن الحجم بعد نجاح الـ PTY resize بس (أو صفّ الكاش عند الخطأ عشان المحاولة الجاية تعيد)، و `tracing::warn!` على الخطأ في `write` و `resize` بدل `let _ =`.

---

### 🟡 A6 ‏[Minor] مسار إيفنتات التيرمنال بيعدي حدود الـ output bounding: سيل titles/bells/queries بيكوّم إيفنتات غير محدودة

**المكان:** `src/terminal.rs:21-28` ‏(`ForwardListener` بيمرر كل إيفنتات alacritty)، `src/session/spawn.rs:36-39` ‏(كل واحدة `AppEvent::Terminal` على الـ event loop)، التنفيذ في `src/app/event.rs:74-94`

**المشكلة:** بايتات الـ output متدمجة ومحكومة (قائمة 4 MiB + ميزانية 12ms)، بس الـ events **مش متدمجة خالص** — إيفنت heap-allocated لكل escape sequence. برنامج بيبعت سيل مستمر من OSC titles / bells / color queries بيخلي طابور الإيفنتات يكبر والـ UI يعمل شغل لكل إيفنت على حدة (الـ Title لوحدها بتعمل `set_title` على النافذة + طلب redraw)، فيقع التيرمنال تدريجيًا ورا الـ output الحقيقي. وكل ده input تحت سيطرة البرنامج المعروض.

**الـ Fix:** ادمج الإيفنتات القابلة للضياع قبل الـ proxy: آخر `Title`/`ResetTitle` بس لكل pane، و `Bell` واحدة معلقة بالكتير، في slot مشترك صغير الـ listener بيكتب فيه والـ loop بيصرفه.

---

### 🔵 A7 ‏[Suggestion] ‏Ctrl+0 ‏(zoom reset) بيرمي font size اللي المستخدم مظبطه في الـ config — وبيحفظ الخسارة

**المكان:** `src/app/keyboard.rs:82` ‏— `ZoomReset => self.update_font_size(DEFAULT_FONT_SIZE)` ‏(‏`DEFAULT_FONT_SIZE = 14.5` في `src/ui/settings.rs:3`)، مقابل توثيق الأكشن نفسه في `src/input/keys.rs:23-24`: ‏"Go back to the **configured** font size"

حجم الخط المحمّل من config المستخدم عند بدء التشغيل مش محفوظ في أي حتة. لو الـ config فيه `"font_size": 20`، كبسة Ctrl+0 بتقفز لـ 14.5 مش 20، ولأن `update_font_size` بيظبط `config_dirty` ‏(`src/app/window.rs:360-367`)، الخروج بعدها بيحفظ 14.5 فوق الـ 20 بتاعة المستخدم نهائيًا. **تنبيه:** لو قصد المنتج «reset للمصنع» فدي مشكلة تسمية/توثيق بس؛ لو التوثيق هو الـ spec، احفظ الـ `config.font_size` بتاع البداية في `App` وارجعله.

---

### 🔵 A8 ‏[Suggestion] كاش صفوف الـ snapshot مفتاحه hash ‏64-bit مخصص — تصادم نظري بيرسم صف غلط

**المكان:** `src/terminal.rs` ‏(`snapshot_rows: HashMap<u64, LineData>` و `KeyHasher` — folded-multiply مخصص مش cryptographic)

الصفوف اللي مفتاحها بيتصادم بتتشارك نفس الـ `LineData` فتترسم بيانات صف مكان صف تاني لحد ما المحتوى يتغير. الاحتمالية ضئيلة عمليًا (64-bit)، ودي ملاحظة تصميم مش bug مرصود — تتسجل للشفافية بس، وأي إصلاح (مقارنة تحقق رخيصة عند إعادة الاستخدام) لازم يتقاس أثره على الأداء الأول لأن المسار ده hot.

---

## 4. الـ Security Scan — النتائج بالتفصيل

منهجية الـ skill: الـ finding مبيبقاش `confirmed` غير بـ trust boundary حقيقي (principal أقل ثقة → input مقبول → control مفروض يحمي → نتيجة ملموسة)، والـ severity للـ confirmed بس.

### ✅ Confirmed

#### S1 ‏[Confirmed — Medium] أي output معروض يقدر يبدّل Clipboard النظام بصمت عن طريق OSC 52

**المسار:** بايتات PTY → بارسر `alacritty_terminal` → ‏`Event::ClipboardStore` → ‏`src/app/event.rs:113-115`:

```rust
TermEvent::ClipboardStore(_, text) => {
    // OSC 52 copy (e.g. from tmux/vim over SSH).
    self.clipboard.set_text(text);
}
```

→ ‏`src/ui/clipboard.rs:24-27` ‏(‏`arboard::set_text`). **مفيش أي تأكيد من المستخدم، ولا فحص focus/ownership، ولا حد لحجم الـ payload، ولا gating حسب المصدر** في أي حتة على المسار ده.

**الـ boundary:** الـ clipboard مورد بتاع المستخدم مشترك بين التطبيقات. الـ principal الأقل ثقة هنا أي برنامج شغله مجرد **عرض** output — وبصراحة الكومنت نفسه: ده شامل كود remote (tmux/vim فوق SSH)، أو حتى `cat` لملف غير موثوق فيه الـ sequence. نص المهاجم بيستبدل بصمت أي حاجة المستخدم نسخها، والضرر بيتحقق عند أول paste جاي في تطبيق/شيل/متصفح تاني (الـ paste بتاع Twitty نفسه محمي بـ bracketed paste — التطبيقات التانية لأ).

**ملاحظة عدم التماثل:** اتجاه **القراية** مرفوض عمدًا وبنفس المنطق بالظبط (`src/app/event.rs:117-118` — شوف R1 تحت)، بس اتجاه **الكتابة** اتساب بلا شرط.

**الـ Fix:** prompt عند الـ store (على الأقل للـ panes غير المركزة/الخلفية أو أول استخدام في الجلسة)، أو opt-in في الـ config، + حد لحجم الـ payload (شوف NV3).

#### S2 ‏[Confirmed — Medium] سلسلة الـ Release: مراجع أكشنز قابلة للتحريك بتبني الباينريات المنشورة، والفحص الوحيد سلامة من نفس المصدر على باينري غير موقّع

**المسار (كله في `.github/workflows/build.yml`):** الجوبز اللي بتنتج الأرتيفاكت بتستخدم مراجع mutable: ‏`actions/checkout@v4`، ‏`dtolnay/rust-toolchain@stable` ‏(مرجع **branch** — بيخلي الأكشن دي تتحكم في الكود والـ toolchain جوه الـ build)، ‏`swatinem/rust-cache@v2`، ‏`actions/upload-artifact@v4`. جوب الـ release (مقفولة على tags، ‏`contents: write`) بتنزّل الأرتيفاكتات دي، وبتولّد الـ checksums **من الأرتيفاكتات زي ما وصلت** (`sha256sum twitty-* > SHA256SUMS.txt`)، وبتنشر كله مع بعض بـ `gh release create … artifacts/*`.

**النتيجة:** مرجع أكشن اتحرك أو اتخترق في جوب build بيطلع باينري twitty ملوّث بيتنشر كـ release رسمي **ومعه `SHA256SUMS.txt` سليم بيطابقه تمامًا** — لأن الـ checksum بيتولد **بعد** نقطة الاختراق المحتملة، ومن نفس الـ release، ومن غير أي جذر ثقة مستقل. وجانب المستهلك مؤكد في `README.md:58`: ‏«Builds are not code-signed yet» والـ release «ships a SHA256SUMS.txt» — يعني الـ checksum بيكشف تلف النقل/المرايات بس، مش استبدال الـ release نفسه.

**مقارنة جوه الريبو نفسه:** الأكشن الوحيدة المثبتة بـ SHA كامل هي `rustsec/audit-check@69366f33…` ‏(# v2.0.0) — فالتثبيت كان معروف ومتاح، بس مطبقش على سلسلة الـ build→release. (أكشنز `taiki-e/install-action` المش مثبتة برضه بره النتيجة دي لأن جوب الـ fuzz ‏`continue-on-error` ومش بتغذي الـ release.)

**الـ Fix:** ثبّت كل أكشنز `build-windows`/`build-linux`/`release` بـ SHA كامل (الـ dependabot أصلًا بيغطي `github-actions` أسبوعيًا فالتثبيتات هتفضل حديثة)، ووقّع الأرتيفاكتات (Sigstore/GitHub attestations) عشان التحقق يبقى له جذر مستقل عن صفحة الـ release.

### 🟡 Needs Validation — فرضيات مبنية على source، ناقصها فحص محدد

| # | الفرضية | الحقيقة الناقصة بالظبط | خطة تحقق آمنة |
|---|---|---|---|
| NV1 | ردود استعلامات التيرمنال (`Event::PtyWrite` في `src/app/event.rs:73`) بتتمرر للـ PTY زي ما هي، ومحتواها بيتولد جوه الـ dependency | بايتات `alacritty_terminal 0.26.0` الفعلية لردود DA1/DA2 و cursor-position و DECRQM و kitty queries — وخصوصًا أي **title report** (‏CSI 21t) ممكن يضمّن محتوى المهاجم حطه بـ OSC 0/2 فيوصل لشيل قاعد عند prompt | قراءة `ansi.rs` في نسخة 0.26، أو sandbox: تغذية الاستعلامات دي في نص `cat` وملاحظة سلوك محرر سطر الشيل |
| NV2 | عنوان النافذة/التاب اللي البرنامج بيظبطه بيتحفظ وبيتعرض من غير sanitize ‏(`src/app/event.rs:74-80` → `workspace.rs` → `window.rs:380-385` لـ `winit::set_title`؛ عناوين التابات بتتقطع بالعرض بس في `layout.rs:290-307`) | هل بارسر alacritty بيجرد حروف C0/C1 والـ bidi controls ‏(U+202A–202E, U+2066–2069) من بارامترات OSC ولا لأ، وهل winit/cosmic-text بيحترموهم لو وصلوا — لو لأ، برنامج remote يقدر يزوّر/يعكس العنوان الظاهر (UI redress منخفض الأثر) | فحص بارسر vte في الـ dependency + test محلي بـ title فيه bidi override |
| NV3 | مفيش حد لحجم payload الـ OSC 52 في كود Twitty ‏(`src/app/event.rs:113-115`) | هل بارسر alacritty بيحدد طول تسلسلات OSC داخليًا ولا لأ — لو لأ، sequence واحدة ضخمة/غير منهية من output معروض = DoS ذاكرة/clipboard ‏(قائمة الـ output محكومة بـ 4 MiB، بس دي بتقيد الـ queueing مش تراكم sequence واحدة جوه البارسر) | قراءة حدود البارسر في 0.26 أو fixture محلي محكوم |
| NV4 | حالة advisories/yanks للديبندنسيز غير قابلة للإثبات من الـ source | نتيجة RustSec advisory DB على الـ 378 package المقفولة في `Cargo.lock` — الـ CI بيشغّل `rustsec/audit-check` بس `cargo audit` متشغلش هنا (مفيش toolchain) فبنأكدش لا «نضيف» ولا «فيه ثغرة» | تشغيل `cargo audit` في بيئة فيها Rust toolchain |
| NV5 | اكتمال صورة وراثة الـ environment عند spawn الشيل معتمد على سلوك الـ dependency | هل `portable-pty 0.9` ‏`CommandBuilder` بيورّث بيئة الأب كاملة افتراضيًا (الكود هنا بيظبط `TERM` وغيره وبيشيل اللي في `INHERITED_TERMINAL_VARS` في `src/pty.rs:167-180` بس) | مراجعة docs/source الـ portable-pty 0.9 — وملحوظة: ده للاكتمال بس، لإن الوراثة نفسها مش boundary violation (شوف R3) |

### ❌ Rejected — claims اتفحصت واتفندت

- **R1 — «OSC 52 يخلي برنامج يقرا/يسرب الـ clipboard»:** مرفوض بالتصميم — `src/app/event.rs:117-118`: ‏`ClipboardLoad(..) => {}` بكومنت صريح *"OSC 52 reads are refused: any program could silently exfiltrate the clipboard."* ومفيش أي مسار تاني لـ `Clipboard::text()` غير الـ paste اللي المستخدم بيبدأه.
- **R2 — «الـ scheme allow-list للروابط (OSC 8 والـ plain URLs) قابلة للتجاوز»:** فشل قدام `src/links.rs:86-94`: أي حرف control أو whitespace **في أي حتة** في الـ URL بيرفضها (بيقتل حيل الـ leading-space والـ newline/CRLF المدمج)، الـ scheme بيتقارن `eq_ignore_ascii_case` بقايمة `[http, https, file, mailto, ftp]` وباقي فاضي بيترفض؛ والفتح (`src/links.rs:97-110`) بيعيد الفحص وبعدين `Command::new(program).arg(url)` — أرجومنت واحد، **من غير shell**، والـ URL لازم يبدأ بـ scheme مسموح فمفيش option injection؛ والفتح نفسه محتاج Ctrl+Click واعي من المستخدم (`src/app/mouse.rs`). ‏(بقايا مقصودة مش تجاوز: `file://` مسموح بيه عمدًا.)
- **R3 — «الـ spawn بتاع الشيل (وراثة ‏LD_PRELOAD وغيره، بارسينج ‏TWITTY_SHELL/$SHELL، الـ cwd) أسطح injection»:** مفيش boundary بتتعدى — الشيل ووسائطه وبيئته جايين من **جلسة المستخدم نفسه اللي شغّل Twitty**، وشيل المستخدم في تيرمناله هو نفس الـ principal. والـ cwd بتاع pane جديد بيتاخد من شيل المستخدم نفسه عن طريق `/proc/<pid>/cwd` وبيتستخدم بس لو دايركتوري موجود (`src/pty.rs:178-180`). وليستة التجريد في `src/pty.rs:15-45` شغلها الصح أصلًا: بتشيل حالة بروتوكولات التيرمنال الأب (TMUX, KITTY_*, WEZTERM_*, ALACRITTY_*) عشان متتسربش للطفل.
- **R4 — «ملف config ممكن ينفذ أوامر أو يهرب من صندوقه»:** الـ schema مفيهاش أي حقل command/hook/path تنفيذ أصلًا (`src/config.rs:9-31`)؛ القيم بتتظبط ranges ‏(`sanitize`: فونت 8–48، شفافية 0.2–1.0، واللانهائي → defaults)، الألوان من `parse_hex` محكوم (3 أو 6 hex)، الثيمات المجهولة بتقع للافتراضي بتنبيه، و `font_family` بيتماتش بالاسم مع فونتات مثبّتة بس — عمره ما بيتفتح كمسار.
- **R5 — «الـ paste يقدر يهرب من الـ bracketed paste ويحقن أوامر»:** ‏`src/input/keys.rs` ‏(`encode_paste`): في وضع الـ bracketed paste كل ‏ESC بتتشال من جسم الـ paste قبل التغليف بـ ‏`ESC[200~ … ESC[201~` — مصمم صراحة عشان المحتوى مينهيش القوس بدري ويحقن أوامر. الفرع غير المغلف بيتحول newlines لـ CR وبيتوصل بس من فعل paste بتاع المستخدم نفسه من clipboard بتاعه.
- **R6 — «ثغرات memory-safety في ‏unsafe/FFI/indexing على أطوال جاية من الـ escape stream»:** مفيش `unsafe` أصلًا يتفحص (`forbid(unsafe_code)` — grep على الريبو كله بيرجع السطرين دول بس)؛ استخدام `bytemuck` الوحيد آمن (derives ‏`Pod`/`Zeroable` على structs ‏f32 بتاعة التطبيق و `bytes_of`/`cast_slice` على بيانات vertex بتاعته هو)؛ والـ indexing على مسار البيانات غير الموثوقة اتقرا سطر بسطر ولقى محكوم (`render/text/segment.rs` بـ `partition_point` و `saturating_sub`، `shaper.rs` بفهارس داخل حلقات `start..end`، `terminal.rs` search spans متكلامبة بـ `.min(last_col)` وحارس `index >= 256` لبرنامج الألوان). أي دعوى panic من fuzz input أبعد من القراية الثابتة محتاجة تشغيل — وده بره النطاق هنا، وهو بالظبط اللي الـ fuzz targets معمولة عشانه.

---

## 5. ‏✅ Checked & OK — اللي اتفحص وطلع سليم

- **صفر `unsafe`** في كود المشروع كله (`#![forbid(unsafe_code)]` في الليب والباينري) — الـ FFI كله محبوس جوه مكتبات ناضجة بواجهات آمنة.
- **انضباط الـ panics:** ‏7 ‏`unwrap()` في `src/` — ‏5 في tests، و2 في `src/render/row/mod.rs:160,168` محروسة بفلتر `sized(...).is_some()` قبل المقارنة. صفر `panic!`/`unreachable!` في كود الإنتاج.
- **حارس الـ grid:** الأبعاد الصفرية بتتكلامب لـ 1 قبل ما توصل alacritty (`Terminal::new`/`resize` بـ `.max(1)`) — مفيش panic جريد فاضي.
- **ضغط الـ output معمول صح:** قائمة PTY محكومة بـ 4 MiB مع backpressure على القارئ (`src/pty.rs:65,74-90`) وميزانية بارسينج 12ms لكل إيفنت (`src/app/event.rs:21,25-48`) — `cat` لملف ضخم لا بيجمد الـ UI ولا بيكبر الذاكرة بلا حد.
- **قفل الـ mutexات متعالج:** الـ OutputQueue بتتعامل مع الـ poisoning في كل مساراتها (`push` بـ `let Ok else`، `take` بـ `unwrap_or_default`، `close` بـ `if let Ok`) ونفس النمط في `WARNINGS` بتاعة الـ config.
- **الـ config نموذجي:** ‏sanitize بكلابسات محدودة، حفظ atomic بـ tmp+rename، وملف تالف بيتحفظ ومبيتكتبش فوقه (`keep_file`) — باستثناء ثغرة A4 الضيقة.
- **البحث آمن:** استعلام المستخدم بيتهرب لـ literal regex ‏(`src/terminal.rs:141-152`) وعد المطابقات متكاب عند 100,000.
- **مفيش auto-updater:** مفيش أي كود شبكة/تنزيل/تحديث في `src/` — التحديث تنزيل يدوي من GitHub Releases، فبالتالي S2 هي قصة التحديث كلها.
- **نضافة CI (غير S2):** صلاحيات افتراضية `contents: read`، والـ `contents: write` الوحيدة محصورة في جوب الـ release المقفولة على tags؛ مفيش `pull_request_target` ولا triggers من issues/comments، ومفيش secrets في جوبز الـ PR.
- **الـ supply chain (غير S2):** ‏`Cargo.lock` ملتزم بيه (v4 بـ checksums، ‏378 package)، ‏dependabot أسبوعي لـ cargo والأكشنز، و `deny.toml` بيمنع الـ wildcards والريجستريات والـ git sources المجهولة وبيحصر المصادر في crates.io وبيسمح بتراخيص permissive بس (ثغرات بس في إن الـ advisories مفوضة لجوب cargo-audit بدل cargo-deny، وإن `multiple-versions = "warn"` — hardening grade).
- **تغطية الـ tests والـ fuzz:** ‏~217 test (منهم proptest و regression suites مخصصة للـ audit)، وهدفين fuzz بيغطوا أخطر سطحين للبايتات غير الموثوقة: بارسينج التيرمنال (`terminal_bytes`: ‏process/resize/snapshot/link_at) وتشكيل الصفوف (`shape_rows`: ‏build_row/extents/quads). **فجوات تغطية متسجلة:** بارسينج الـ config، قرار `is_allowed_scheme`/`open_url` نفسه، ‏`encode_paste` وإنكودرز المفاتيح، و spawn الـ PTY — والـ CI بيشغل كل هدف fuzz ‏60 ثانية بس والجوب `continue-on-error`.
- **العناوين والألوان:** ردود OSC 4/10/11/12 و text-area-size بتتولد من قيم التيرمنال نفسه بأرقام/hex جوه فريمينج escape ‏(`src/app/event.rs:89-111`)، و OSC 7 مش بتتاخد أصلًا (الـ cwd من `/proc`)، و OSC 133 مش موثوقة ولا بتتفاعل.

---

## 6. ترتيب الإصلاح المقترح (Prioritized)

1. **A1** — فك ربط موت الشيل بالـ EOF (waiter thread مستقل + shutdown للـ reader): بيقضي على zombies و panes عالقة وتسريبات دائمة.
2. **A2** — حد لقناة إدخال الـ PTY (`sync_channel` + ‏try_send): بيقفل نمو ذاكرة لا نهائي مدفوع جزئيًا من الـ stream غير الموثوق.
3. **S2** — تثبيت أكشنز الـ build/release بـ SHA + توقيع/attestation للأرتيفاكتات: جذر الثقة الوحيد للمستخدمين اللي بينزلوا الباينري.
4. **A4** — تمييز `NotFound` عن باقي أخطاء قراية الـ config: سطرين وبيمنعوا ضياع ملف مستخدم بصمت.
5. **S1** — ‏gating/prompt لكتابة OSC 52 + كاب للحجم (مع NV3): بيسد عدم التماثل مع رفض القراية.
6. **A5 + A3** — ‏logging وعدم ابتلاع أخطاء الـ PTY، و reap صريح في مسار فشل الـ spawn.
7. **A6** — دمج إيفنتات العنوان/الجرس قبل الـ event loop.
8. **NV1/NV2** — التحقق من ردود title-report وتجريد عناوين النوافذ في نسخة alacritty المثبتة.
9. **A7 + A8** — حسب نية المنتج وتكلفة الأداء على الترتيب.

---

## 7. حدود التدقيق (Limitations)

- **مفيش تشغيل:** ‏`cargo check/test/audit/fuzz` متشغلوش — مفيش Rust toolchain في البيئة. كل الـ confirmed من تتبع source كامل؛ واللي محتاج سلوك dependency أو تشغيل اتسجل `needs_validation` بالحقيقة الناقصة وخطة التحقق.
- الـ dependencies (alacritty_terminal 0.26، portable-pty 0.9، wgpu، cosmic-text) اتراجعت من ناحية **الاستخدام والتكامل** بس، مش source audit كامل لكرates الطرف التالت.
- جولة واحدة مش إثبات غياب: الـ fuzz targets بتاعة المشروع نفسه (60 ثانية في CI) والاستخدام الحقيقي هما اللي بيكملوا الصورة مع الوقت.
