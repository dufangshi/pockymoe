/* Creates the `<style data-theme-pack>` element while main.tsx evaluates its imports:
   after the host's stylesheets and before the shared UI injects its own (and before
   any lazily loaded CSS). Theme packs used to be part of the host stylesheet, so
   this keeps their place in the cascade: a pack rule still loses a specificity tie
   to the shared UI, as it did. main.tsx imports this module before `./app`. */

if (typeof document !== 'undefined' && !document.querySelector('style[data-theme-pack]')) {
  const style = document.createElement('style');
  style.dataset.themePack = '';
  document.head.append(style);
}

export {};
