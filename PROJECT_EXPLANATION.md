# RTL Terminal - Project Explanation

## فكرة المشروع

RTL Terminal هو تطبيق Terminal لسطح مكتب Windows مبني بـ Tauri و React. الهدف منه تشغيل Terminal حقيقي داخل نافذة Desktop، مع دعم أفضل للنصوص المختلطة بين العربي والإنجليزي، وتشغيل أدوات تفاعلية مثل `opencode` وواجهات TUI بشكل أقرب للترمنال الطبيعي.

المشروع بدأ بمحاولة رسم Terminal مخصص، ثم تم نقله إلى `xterm.js` لأن أدوات CLI الحديثة تحتاج دعم قوي لتسلسلات ANSI، alternate screen، الأسهم، الاختصارات، الألوان، والـ scrollback.

## التقنيات المستخدمة

- Tauri v2 لتطبيق سطح المكتب وبناء نسخة Windows.
- React 19 لواجهة التطبيق.
- Vite للبناء والتطوير السريع.
- TypeScript لواجهة المستخدم.
- Rust للـ backend والتعامل مع الـ PTY.
- `portable-pty` لتشغيل shell حقيقي داخل pseudo terminal.
- `@xterm/xterm` لمحاكاة Terminal كاملة في الواجهة.
- `@xterm/addon-fit` لضبط حجم الترمنال تلقائيا مع حجم النافذة.

## هيكل الملفات المهم

- `src/components/XtermTerminal.tsx`: واجهة الترمنال الأساسية المبنية على xterm.js.
- `src/App.tsx`: نقطة عرض التطبيق، وتعرض مكون الترمنال.
- `src/styles.css`: تنسيق الشاشة الكاملة وشكل الترمنال.
- `src-tauri/src/pty.rs`: تشغيل وإدارة جلسة الـ PTY وقراءة/كتابة بيانات الترمنال.
- `src-tauri/src/lib.rs`: أوامر Tauri التي يستدعيها الـ frontend مثل بدء الجلسة والكتابة وتغيير الحجم.
- `src-tauri/tauri.conf.json`: إعدادات تطبيق Tauri والـ bundle.
- `.github/workflows/windows-build.yml`: GitHub Actions لبناء Release كامل لويندوز.
- `scripts/normalize-config.cjs`: سكربت لتظبيط encoding ملفات الإعداد قبل البناء.

## طريقة العمل داخليا

عند فتح التطبيق، مكون `XtermTerminal` ينشئ Terminal داخل الصفحة باستخدام xterm.js. بعد ذلك يستدعي أمر Tauri باسم `start_terminal`، وهذا الأمر يشغل shell حقيقي على Windows من خلال `portable-pty`.

أي كتابة من المستخدم داخل xterm تنتقل إلى Rust عن طريق الأمر `write_terminal`. Rust يكتب النص في الـ PTY، والـ PTY يرجع الخرج مرة أخرى إلى الواجهة عن طريق event باسم `terminal://data`. الواجهة تستقبل هذا الخرج وتكتبه داخل xterm.

عند تغيير حجم النافذة، `FitAddon` يحسب عدد الأعمدة والصفوف المناسب، ثم يتم إرسال الحجم الجديد إلى backend عن طريق `resize_terminal`.

## دعم العربي والإنجليزي

المشروع يستهدف أن يكون النص العربي RTL والإنجليزي LTR بدون تكسير النص المختلط قدر الإمكان. الاعتماد الحالي في العرض الأساسي على قدرات المتصفح و xterm.js والخط المستخدم.

النقطة المهمة أن التطبيق لم يعد يرسم كل حرف يدويا كما كان سابقا، لأن الرسم اليدوي كان يسبب مشاكل مع برامج تفاعلية مثل `opencode`. استخدام xterm.js يعطي توافق أعلى مع أدوات CLI الحديثة، مع الاحتفاظ بمحاولة جعل الشكل مناسب للنصوص المختلطة.

## التعامل مع Ctrl+C

على Windows، بعض برامج Node أو أدوات TUI لا تتعامل دائما مع إرسال byte واحد مثل `\x03` داخل ConPTY بنفس سلوك Terminal عادي. لذلك المشروع يحتوي على أمر `interrupt_terminal`.

هذا الأمر يعمل كالتالي:

1. يرسل Ctrl+C طبيعي للـ PTY.
2. ينتظر مدة قصيرة.
3. لو البرنامج الابن الذي يعمل داخل shell ما زال معلقا، يتم إنهاء العمليات الأبناء فقط.
4. يتم ترك shell نفسه شغال، لذلك يرجع المستخدم إلى prompt بدون restart كامل للتطبيق.

الهدف من هذا السلوك أن `Ctrl+C` يخرج من الأمر الحالي مثل الترمنال الطبيعي، وليس أن يعيد تشغيل الجلسة كلها.

## الاختصارات والتنقل

- `Ctrl+C`: محاولة قطع الأمر الحالي والرجوع إلى prompt.
- `Ctrl+D`: إرسال EOF للـ shell أو البرنامج الحالي.
- السهم لأعلى: يرجع للأوامر السابقة من history عندما يدعمها shell.
- السهم لأسفل: يتحرك للأمر التالي في history.

الأسهم لا يتم التعامل معها يدويا في React، بل تترك لـ xterm والـ PTY حتى تعمل بشكل طبيعي مع shell وبرامج TUI.

## تشغيل المشروع محليا

تثبيت dependencies:

```bat
npm install
```

تشغيل واجهة Vite فقط:

```bat
npm run dev
```

تشغيل تطبيق Tauri في وضع التطوير:

```bat
npm run tauri:dev
```

ملاحظة: تشغيل Tauri محليا على Windows يحتاج Rust و Visual Studio Build Tools مع مكون Visual C++، لأن Rust MSVC يحتاج `link.exe`. لو الجهاز لا يحتوي على هذه الأدوات، استخدم GitHub Actions للبناء.

## بناء نسخة production

بناء الواجهة فقط:

```bat
npm run build
```

بناء تطبيق Tauri محليا:

```bat
npm run tauri:build
```

## البناء عن طريق GitHub Actions

يوجد workflow جاهز في:

```text
.github/workflows/windows-build.yml
```

الـ workflow يعمل على `windows-latest` ويقوم بالخطوات التالية:

1. تحميل الكود من GitHub.
2. تثبيت Node.js 24.
3. تثبيت Rust stable مع target ويندوز MSVC.
4. تثبيت dependencies عن طريق `npm ci`.
5. تشغيل سكربت normalization لملفات الإعداد.
6. بناء الواجهة عن طريق `npm run build`.
7. فحص Rust backend عن طريق `cargo check`.
8. بناء تطبيق Tauri كـ Windows NSIS installer.
9. رفع build artifact.
10. إنشاء GitHub Release تلقائي يحتوي على ملفات Windows الناتجة.

لتشغيله، اعمل commit و push على branch `main` أو `master`:

```bat
git add .
git commit -m "Update RTL Terminal"
git push --set-upstream origin main
```

بعد انتهاء GitHub Actions، افتح صفحة Releases في GitHub وستجد release جديد باسم قريب من:

```text
RTL Terminal Windows build <run_number>.<run_attempt>
```

## ملاحظات مهمة

- مجلد `kitty/` ليس جزءا من build الحالي ويجب ألا يتم إضافته بالخطأ إلى Git.
- المشروع يبني Windows فقط في GitHub Actions.
- لو ظهرت رسالة عن Node.js 20 في GitHub Actions، فالـ workflow مضبوط بالفعل على Node.js 24 ويحتوي على `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24`.
- لو فشل البناء بسبب JSON parsing، تأكد من تشغيل `scripts/normalize-config.cjs` ضمن workflow كما هو موجود حاليا.

## الهدف الحالي من المشروع

الوصول إلى Terminal Windows بسيط وشكله طبيعي، بدون واجهة زائدة أو ألوان غريبة، يعمل مع:

- أوامر Windows العادية.
- برامج CLI التفاعلية.
- أدوات TUI مثل `opencode`.
- نصوص عربية وإنجليزية مختلطة بأقل مشاكل ممكنة.
- اختصارات أساسية مثل `Ctrl+C` والأسهم بشكل قريب من الترمنال الطبيعي.
