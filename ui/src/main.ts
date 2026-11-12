import { createApp } from 'vue';
import { createPinia } from 'pinia';
import ShellApp from '@/shell/ShellApp.vue';
import '@/assets/main.css';

createApp(ShellApp).use(createPinia()).mount('#app');
