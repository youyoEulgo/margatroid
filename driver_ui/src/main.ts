import { createApp } from 'vue';
import { createPinia } from 'pinia';
import DriverApp from './DriverApp.vue';
import 'katex/dist/katex.min.css';
import '@/assets/main.css';
import './driver.css';

createApp(DriverApp).use(createPinia()).mount('#app');
