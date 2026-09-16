const header = document.querySelector("[data-header]");

const updateHeader = () => {
  header?.classList.toggle("is-scrolled", window.scrollY > 12);
};

updateHeader();
window.addEventListener("scroll", updateHeader, { passive: true });

const faqItems = [...document.querySelectorAll(".faq-list details")];
faqItems.forEach((item) => {
  item.addEventListener("toggle", () => {
    if (!item.open) {
      return;
    }

    faqItems.forEach((other) => {
      if (other !== item) {
        other.open = false;
      }
    });
  });
});

document.querySelectorAll("[data-download]").forEach((link) => {
  const architecture =
    link.dataset.downloadArch === "intel" ? "Intel" : "Apple Silicon";
  link.setAttribute(
    "aria-label",
    `Download DuckDisk v0.6.3 for ${architecture} macOS`
  );
});
