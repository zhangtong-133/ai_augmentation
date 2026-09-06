import type { MetadataRoute } from "next";

export default function manifest(): MetadataRoute.Manifest {
  return {
    name: "Personal AI Augmentation System",
    short_name: "Personal AI",
    description: "Personal knowledge, learning, and agent dashboard.",
    start_url: "/",
    display: "standalone",
    background_color: "#101a17",
    theme_color: "#101a17",
    icons: [
      {
        src: "/icon.svg",
        sizes: "any",
        type: "image/svg+xml",
        purpose: "any",
      },
    ],
  };
}
