import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
// Fontene bakes inn i dist (Vite bundler woff2-filene), så appen virker uten nett.
// Samme akser som den tidligere Google Fonts-lenken: Fraunces med opsz og wght
// (normal og kursiv), Hanken Grotesk og Spline Sans Mono med wght.
import "@fontsource-variable/fraunces/opsz.css";
import "@fontsource-variable/fraunces/opsz-italic.css";
import "@fontsource-variable/hanken-grotesk/wght.css";
import "@fontsource-variable/spline-sans-mono/wght.css";
import "./theme.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 30_000, refetchOnWindowFocus: false },
  },
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </QueryClientProvider>
  </React.StrictMode>,
);
