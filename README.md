# AuraStudio

Logiciel de dessin / peinture / animation / BD en Rust, construit from scratch avec une approche "inspired rewrite" (lire la logique de Krita, OpenToonz et PhotoCraft, la réimplémenter proprement en Rust idiomatique — pas de traduction de code).

Objectif : un brush engine simple et intuitif à la CSP/Procreate, sans la complexité inutile du brush engine de Krita.

## Stack technique

| Composant | Choix |
|---|---|
| Fenêtre + events | `winit` 0.30 |
| Rendu GPU | `wgpu` 0.20, backend **Vulkan** sur Windows/Linux, **Metal** sur macOS |
| Input stylet (Windows) | Hook natif `WM_POINTER` via `windows-rs` (bypass de l'API Touch de winit) |
| Build / CI | GitHub Actions, 100% en ligne, rien installé en local |

## Discipline de travail

Chaque brique doit être solide et mesurée avant de passer à la suivante. Checklist de validation :
- Mémoire GPU stable, pas de fuite
- 60 fps constant
- Pas d'allocation par frame
- Latence stroke la plus faible possible

Contre-exemple à ne pas reproduire : PhotoCraft qui pèse 1,5 Go en mémoire pour un canvas vide.

---

## Ce qui a été fait

### Brique 1 — Fenêtre + GPU ✓
- Fenêtre winit qui s'ouvre, resize propre, 60fps
- `ControlFlow::Wait` + redraw uniquement sur event → **0% CPU au repos**
- Backend Vulkan explicite → **~150 Mo RAM** à vide (vs ~400 Mo en DX12)
- Build release ~8,5 Mo, aucune dépendance externe (tout statiquement lié)

### Brique 2 — Canvas GPU ✓
- Texture 2048×2048 RGBA8, centrée et proportionnée via un uniform buffer (`canvas_size` / `window_size`) dans le shader WGSL
- S'adapte proprement au resize de la fenêtre

### Brique 3 — Brush engine basique (en cours)
- Brush rond, rayon fixe 25px, opacité réglable (pas encore de hardness ni de taille dynamique)
- `stroke_line` interpole entre deux positions pour éviter les trous pendant un mouvement rapide
- **Dirty rect** : on n'upload vers le GPU que la zone réellement modifiée par le trait, pas tout le canvas (passage de 16 Mo/event à quelques Ko/event) → latence divisée par ~25%

### Brique 4 — Input stylet natif WM_POINTER (vient d'être codée, pas encore testée)
- Constat : l'API `Touch`/`Force` standard de winit n'est pas conçue pour du stylet haute fréquence → pression saccadée, latence visible (~3-4 cm sur écran 14")
- Même limitation documentée côté PhotoCraft (pression Windows uniquement, macOS/Linux bloqués par winit) et historiquement chez Blender
- Solution : hook direct de la `wndproc` Windows (`SetWindowLongPtrW`) + lecture brute de `WM_POINTERDOWN/UPDATE/UP` via `GetPointerPenInfo`, en bypassant complètement winit pour cette partie
- **Module `stylus.rs` totalement indépendant** : il ne connaît ni le brush, ni le canvas, ni wgpu — il expose juste des `StylusEvent { x, y, pressure, phase }`. `app.rs` fait la colle. Objectif : pouvoir retoucher n'importe quelle brique dans 6 mois sans casser les autres.
- Sur macOS/Linux : stub vide, fallback sur le `Touch` winit standard (même limite que PhotoCraft)

---

## Prochaines étapes

### 1. Valider et stabiliser la brique stylet (priorité immédiate)
- Builder, tester sous Windows, corriger les erreurs de compilation `windows-rs` probables (API bas niveau jamais compilée localement)
- Mesurer : latence visuelle, fluidité de la pression, comportement avec Windows Ink activé ou désactivé côté pilote Wacom
- Objectif de validation : latence quasi imperceptible, pression fluide sans saccade

### 2. Brush engine v2
- Hardness (dureté du bord du brush)
- Taille dynamique (liée à la pression ou réglable)
- Pool de dabs pré-alloué pour éviter toute allocation pendant le trait

### 3. Système de calques
- Composite GPU uniquement (pas de copie CPU)
- Dirty flag par calque (ne recomposer que ce qui a changé)
- Undo/redo en delta, pas en snapshot complet

### Plus tard
- Color picker RGB/HSV (via egui une fois intégré)
- Arc/curve tool, caméra d'animation, fill gap avancé (inspirés du cahier des charges Krita/OpenToonz initial)

---

## Notes de workflow

- Tout le code est édité et commité directement sur GitHub.com (`Create new file` / éditeur web), aucun outil installé en local
- Chaque modification déclenche un build GitHub Actions sur Ubuntu, Windows et macOS en parallèle (`fail-fast: false`)
- Chaque commit a un nom court (< 50 caractères) décrivant précisément le changement
