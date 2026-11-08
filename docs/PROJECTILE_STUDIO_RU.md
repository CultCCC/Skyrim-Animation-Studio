# SDS Projectile Studio

GUI-конструктор цепочек проджектайлов для [NewProjectilesTMP](https://github.com/fenix31415/NewProjectilesTMP) (fenix31415). Собран в workspace SDS Anim Studio и использует его крейты: `vfs` (профиль MO2 + BSA), `records` (индекс записей), `i18n` (RU/EN) и тему.

- Крейт `crates/proj` — модель конфига, проверки, цепочки, фигуры. Тесты: `cargo test -p sds-proj`.
- Крейт `crates/proj-studio` — GUI, exe `sds-projectile-studio.exe`.
- Сборка: `CARGO_TARGET_DIR=target/proj cargo build --offline --release -p sds-projectile-studio` (отдельный target, чтобы не блокировать сборку студии).

## Что умеет

- **Профиль MO2.** Загружает порядок загрузки (≈1,3 с на SDS) и показывает все `Data/HomingProjectiles/*.json` с модом-источником. Записи PROJ/SPEL/MGEF/EXPL/WEAP/KYWD/NPC_ ищутся по EditorID, имени и FormID. Настройки MO2 при первом запуске берутся из `sds_anim_studio.json`, если он лежит рядом.
- **Цепочки.** У каждого триггера своя дорожка слева направо: условия → функции → HomingData / эмиттер / спутник / мультикаст → группа спавна и заклинание → выпущенный снаряд → триггеры, которые снова его ловят (при `callTriggers`). Петли, которые снова ведут в тот же триггер, помечаются. Снаряды, которые ловят другие конфиги загрузки, подписаны.
- **Редактирование.** Справа типизированные формы с подсказками по каждому полю:
  - поле со значением по умолчанию не пишется в JSON, пока его не изменить; ↺ возвращает умолчание;
  - выбор записи (🔍) создаёт ключ `key_<EditorID>` в FormIDs;
  - новые экземпляры создаются кнопкой «+» прямо из ссылки;
  - переименование ключа обновляет все ссылки на него;
  - есть шаблоны цепочек, отмена (Ctrl+Z / Ctrl+Y) и 3D-превью фигуры, точки считаются по формулам Positioning.cpp.
- **Проверки:** неизвестные или неправильно вложенные поля, неверные ссылки, FormID, которых нет в загрузке, неверный тип записи, обязательные поля (`size`, `rotationTime`, `cursorAngle`, `roundingR`, `rotationTarget`), известные ошибки из README.
- **Экспорт** (Ctrl+S) пишет в `<Output-мод>\HomingProjectiles\<имя>.json`. Исходные моды не изменяются. Если в Output уже есть экспорт файла, при открытии загружается он.

## Два формата условий

| | Установленная в SDS DLL (апрель 2024) | GitHub master |
|---|---|---|
| `conditions` | объект `{"ProjBaseIsFormID": "key_x"}` | массив `[{"type": …, "formID": …, "invert", "OR"}]` |
| НЕ / ИЛИ | нет | есть |

Формат «Авто» определяется по строкам установленной DLL. Читаются оба формата.

## Харнесс

`sds-projectile-studio.exe --screenshot out.png [--open SDSFoo.json] [--select trigger:0|group:key|homing:key|mc:key|emitter:key|follower:key|formid:key] [--tab chains|json|issues] [--lang en] [--new X.json --add-chain homing,recast,shotgun,emitter,followers]`

Строки `ISSUE …` в stderr — найденные проблемы.
