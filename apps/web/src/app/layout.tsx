import type { Metadata, Viewport } from "next";
import type { ReactNode } from "react";
import { ServiceWorkerRegistration } from "@/components/service-worker-registration";
import "./globals.css";

export const metadata: Metadata = {
  title: "Personal AI",
  description: "Your private knowledge, learning, and agent workspace.",
  applicationName: "Personal AI",
  manifest: "/manifest.webmanifest",
};

export const viewport: Viewport = {
  themeColor: "#101a17",
  colorScheme: "dark",
};

export default function RootLayout({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="zh-CN">
      <body>
        {children}
        <ServiceWorkerRegistration />
      </body>
    </html>
  );
}
