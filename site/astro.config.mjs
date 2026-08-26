import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';

export default defineConfig({
  site: 'https://syncplane.midhunpm.in',
  output: 'static',
  integrations: [sitemap()],
});
