import { createApp } from 'vue';
import { createPinia } from 'pinia';
import DriverApp from './DriverApp.vue';
import '@/assets/main.css';
import './driver.css';

createApp(DriverApp).use(createPinia()).mount('#app');
