// The preview opens links by catching clicks on HTML <a> elements; a link
// inside an SVG (the Learn page's lesson tree) is an SVG <a> and is not one.
// Catch those first and replay each as a click on an ordinary link.
window.addEventListener(
  "click",
  (event) => {
    const link = event.target instanceof Element ? event.target.closest("a") : null;
    if (!(link instanceof SVGAElement)) return;
    const href = link.getAttribute("href") || link.getAttribute("xlink:href");
    if (!href) return;
    event.preventDefault();
    event.stopPropagation();
    const proxy = document.createElement("a");
    proxy.setAttribute("href", href);
    proxy.style.display = "none";
    document.body.appendChild(proxy);
    proxy.click();
    proxy.remove();
  },
  true,
);
